//! File-backed `BlockDevice`. Used for disk images, raw `/dev/diskN` reads,
//! anything that std::fs::File can address.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

/// A file opened as a block device.
///
/// # Reads do not take the lock; writes do
///
/// A read was `seek` then `read` under a mutex, which made the file's
/// cursor shared state: two threads reading different offsets had to
/// take turns, not because the device could not serve them at once but
/// because one would have moved the other's cursor.
///
/// On Unix the cursor is not involved at all — `pread` takes the offset
/// as an argument — so reads run without the lock and genuinely overlap.
///
/// On Windows the equivalent (`seek_read`) *does* move the file
/// pointer, so the lock stays there. Same behaviour, one platform
/// paying for it.
///
/// Writes keep the lock on both, because `write_at` is still `seek` plus
/// `write_all` and a partial write must not have another writer's seek
/// land in the middle of it.
pub struct FileDevice {
    file: File,
    /// Held for writes only — see the type's own note. `()` rather than
    /// the file, so that a reader physically cannot be made to wait on
    /// it by a later edit.
    write_lock: Mutex<()>,
    size: u64,
    writable: bool,
}

impl FileDevice {
    /// Open read-only.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file,
            write_lock: Mutex::new(()),
            size,
            writable: false,
        })
    }

    /// Open read-write. Errors if the path is not writable.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file,
            write_lock: Mutex::new(()),
            size,
            writable: true,
        })
    }

    /// Open read-write if possible, fall back to read-only otherwise.
    pub fn open_best_effort<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        match Self::open_rw(p) {
            Ok(d) => Ok(d),
            Err(_) => Self::open(p),
        }
    }
}

impl FileDevice {
    /// One positioned read, returning what it got.
    ///
    /// Unix: `pread`, which does not touch the file cursor, so this
    /// needs no lock and concurrent readers overlap.
    #[cfg(unix)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::unix::fs::FileExt;
        Ok(self.file.read_at(buf, offset)?)
    }

    /// Windows: `seek_read` DOES move the file pointer, so the lock is
    /// still required here. The interface is the same and only this
    /// platform pays.
    #[cfg(windows)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::windows::fs::FileExt;
        let _guard = self.write_lock.lock().unwrap();
        Ok(self.file.seek_read(buf, offset)?)
    }
}

impl BlockRead for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        // A SHORT READ IS AN ERROR NAMING WHAT WAS ASKED FOR AND WHAT
        // ARRIVED, not a smaller answer: a caller that asked for a block
        // and got half of one cannot tell the difference from bytes.
        let mut total = 0usize;
        while total < buf.len() {
            let n = self.read_once(offset + total as u64, &mut buf[total..])?;
            if n == 0 {
                return Err(Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: total,
                });
            }
            total += n;
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.size
    }
}

impl BlockDevice for FileDevice {
    /// A write past the end is refused, not an extension.
    ///
    /// `write_all` at a seeked offset EXTENDS a file, and this method
    /// had no bound of its own, so a write straddling the end grew the
    /// backing store while `size_bytes` went on reporting the length
    /// taken at construction -- measured on a 4096-byte file:
    /// `write_at(4094, 8 bytes)` returned `Ok`, the file became 4102
    /// bytes, `size_bytes()` stayed 4096, and `read_at(4096, 6)` then
    /// handed those bytes back. The two halves of one device disagreed
    /// about where it ended, and a caller bounding its reads by
    /// `size_bytes` -- which is what [`crate::CachingDevice`] does,
    /// clamping every block it fetches -- could never reach them.
    ///
    /// `RwBytes` in this crate's own test devices already refuses the
    /// same operation, commenting "a device is not a `Vec`", and the
    /// slice adapters in [`crate::slice`] clamp their window
    /// specifically because this method did not:
    /// `slice_rw_length_is_clamped_and_a_write_past_it_does_not_grow_the_image`
    /// names the file's length on disk as its oracle. This is the same
    /// rule one layer down, where it was missing.
    ///
    /// The alternative -- letting the size move and reopening -- is
    /// what [`BlockRead::size_bytes`]'s contract forbids. See
    /// rust-fs-core#70.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        // `checked_add` because a caller-supplied offset near `u64::MAX`
        // would otherwise wrap and land back inside the device.
        let end = offset.checked_add(buf.len() as u64);
        if end.is_none_or(|end| end > self.size) {
            return Err(Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size: self.size,
            });
        }
        let _guard = self.write_lock.lock().unwrap();
        let mut f = &self.file;
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        let _guard = self.write_lock.lock().unwrap();
        let mut f = &self.file;
        f.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    fn is_writable(&self) -> bool {
        self.writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concurrent readers do not serialise, and none of them sees
    /// another's offset.
    ///
    /// THE BUG THIS REPLACES: reads were `seek` then `read` under one
    /// mutex, so the file cursor was shared state. Two threads reading
    /// different parts of the same image took turns for no reason the
    /// device imposed. Worse, the shape was one edit away from being
    /// wrong rather than merely slow -- drop the lock without moving to
    /// positioned reads and every reader corrupts every other reader's
    /// offset.
    ///
    /// The assertion is on the BYTES rather than on timing: a test that
    /// measured overlap would be a flake on a loaded machine, while a
    /// reader that got another's offset returns the wrong bytes every
    /// time.
    #[test]
    fn many_threads_reading_different_offsets_each_get_their_own_bytes() {
        let path = temp_path("parallel_reads");
        let _c = Cleanup(path.clone());
        // Each 256-byte page filled with its own page number, so a read
        // that landed at the wrong offset is obvious from one byte.
        let mut bytes = Vec::with_capacity(64 * 256);
        for page in 0..64u8 {
            bytes.extend(std::iter::repeat_n(page, 256));
        }
        std::fs::write(&path, &bytes).expect("write the image");

        let dev = std::sync::Arc::new(FileDevice::open(&path).expect("open"));
        let mut handles = Vec::new();
        for page in 0..64u8 {
            let dev = dev.clone();
            handles.push(std::thread::spawn(move || {
                // Several times each, so a thread that raced would have
                // many chances to read somebody else's page.
                for _ in 0..50 {
                    let mut buf = [0u8; 256];
                    dev.read_at(u64::from(page) * 256, &mut buf).expect("read");
                    assert!(
                        buf.iter().all(|b| *b == page),
                        "page {page} came back holding another page's bytes"
                    );
                }
            }));
        }
        for h in handles {
            h.join().expect("a reader panicked");
        }
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique temp path under the system temp dir (no extra dev-deps).
    fn temp_path(tag: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("fs_core_{tag}_{pid}_{n}.bin"))
    }

    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn open_rw_round_trips_write_then_read() {
        let path = temp_path("rw");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0u8; 32]).unwrap();

        let dev = FileDevice::open_rw(&path).unwrap();
        assert!(dev.is_writable());
        assert_eq!(dev.size_bytes(), 32);

        dev.write_at(8, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
        dev.flush().unwrap();

        let mut buf = [0u8; 4];
        dev.read_at(8, &mut buf).unwrap();
        assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn open_rw_errors_on_missing_path() {
        let path = temp_path("missing");
        assert!(FileDevice::open_rw(&path).is_err());
    }

    #[test]
    fn open_best_effort_uses_rw_when_writable() {
        let path = temp_path("best_rw");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0u8; 16]).unwrap();

        let dev = FileDevice::open_best_effort(&path).unwrap();
        assert!(dev.is_writable());
        dev.write_at(0, &[0x11; 4]).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn open_best_effort_falls_back_to_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("best_ro");
        let _g = Cleanup(path.clone());
        std::fs::write(&path, vec![0xEFu8; 16]).unwrap();
        // Read-only permissions force `open_rw` to fail; fall back to `open`.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let dev = FileDevice::open_best_effort(&path).unwrap();
        assert!(!dev.is_writable());
        // Writes are rejected at the read-only layer.
        assert!(matches!(dev.write_at(0, &[0u8; 4]), Err(Error::ReadOnly)));
        // Read still works.
        let mut buf = [0u8; 4];
        dev.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xEF; 4]);
        // Flush on a read-only device is a no-op success.
        dev.flush().unwrap();
    }
}
