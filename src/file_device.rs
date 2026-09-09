//! File-backed `BlockDevice`. Used for disk images, raw `/dev/diskN` reads,
//! anything that std::fs::File can address.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::RwLock;

/// A file opened as a block device.
///
/// # Readers share the lock; writers take it alone
///
/// A read was once `seek` then `read` under a plain mutex, which made
/// the file's cursor shared state: two threads reading different offsets
/// had to take turns, not because the device could not serve them at
/// once but because one would have moved the other's cursor.
///
/// On Unix the cursor is not involved at all — `pread` takes the offset
/// as an argument — so readers hold the lock *shared* and genuinely
/// overlap. On Windows the equivalent (`seek_read`) *does* move the file
/// pointer, so readers there take it exclusively and only that platform
/// pays for the cursor.
///
/// Writers take it exclusively on both, because `write_at` is `seek`
/// plus `write_all` and neither another writer's seek nor a reader may
/// land in the middle of it.
///
/// # THE LOCK IS NOT AN OPTIMISATION, IT IS THE READ/WRITE CONTRACT
///
/// Reads briefly took no lock at all, which read as a natural
/// consequence of positioned reads needing no cursor. It was not: it
/// silently dropped the exclusion between readers and writers that the
/// single mutex had provided, so a read overlapping a `write_at` could
/// observe part of it. `write_all` is permitted to become several
/// `write` calls, and a read is a loop of positioned reads — either
/// split is a window, and the second one does not need the first.
///
/// So a reader holds the lock for the whole of [`FileDevice::read_at`],
/// not for each positioned read inside it. Per-read guards would leave
/// exactly the same hole one level down, and a rarer tear is worse than
/// a common one because nobody can reproduce it.
///
/// # A known limit, stated rather than fixed
///
/// `std::sync::RwLock` does not promise writer preference on every
/// platform, and the read path here is the hot one. A device under
/// sustained parallel reads can therefore make a writer wait longer than
/// a fair queue would. That is a throughput property, not a correctness
/// one, and it is left alone rather than solved with a hand-rolled queue
/// nobody would be able to audit.
pub struct FileDevice {
    file: File,
    /// Shared by readers, exclusive to writers — see the type's own
    /// note. `()` rather than the file, because it orders access rather
    /// than owning the handle: positioned reads need no cursor, so
    /// putting the `File` in here would reintroduce the serialisation
    /// the shared guard exists to avoid.
    io_lock: RwLock<()>,
    size: u64,
    writable: bool,
}

impl FileDevice {
    /// Open read-only.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let size = measure_size(&file)?;
        Ok(Self {
            file,
            io_lock: RwLock::new(()),
            size,
            writable: false,
        })
    }

    /// Open read-write. Errors if the path is not writable.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let size = measure_size(&file)?;
        Ok(Self {
            file,
            io_lock: RwLock::new(()),
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

/// How many bytes the opened handle addresses.
///
/// For a **regular file** this is the metadata length, and 0 is a
/// legitimate answer: an empty file is empty.
///
/// For a **device node** it is not an answer at all. `st_size` is 0 for
/// every block and character device on the platforms this crate ships
/// to, so the size has to be asked for directly. Measured against an
/// 8 MiB backing store:
///
/// | source                    | `metadata().len()` | `lseek(SEEK_END)` | ioctl     |
/// |---------------------------|--------------------|-------------------|-----------|
/// | macOS `/dev/disk9` (blk)  | 0                  | 0                 | 8388608   |
/// | macOS `/dev/rdisk9` (chr) | 0                  | 0                 | 8388608   |
/// | Linux `/dev/loop0` (blk)  | 0                  | 8388608           | 8388608   |
///
/// **`lseek(SEEK_END)` is not the portable fallback it looks like.** It
/// answers 0 on macOS for both node types, so a device opened there
/// would still report itself empty. The ioctl is the only mechanism that
/// answered on both platforms, and it happens to avoid the objection to
/// the seek as well: it does not move the file cursor. That matters
/// here, because `read_once` uses `pread` specifically so reads need no
/// lock -- see this type's own note.
///
/// When the size cannot be measured this returns an error rather than 0,
/// because **a device reporting 0 is not inert, it is invisible.**
/// `read_at` keeps serving real bytes, while
/// [`BlockReadStreamer::read`] returns `Ok(0)` on its first call,
/// [`CachingDevice`] treats every read as past-the-end and caches
/// nothing, and every slice cut from it inherits a parent claiming to be
/// empty. Each of those four failures is silent, which is the one
/// outcome worth refusing outright.
///
/// [`BlockReadStreamer::read`]: crate::BlockReadStreamer
/// [`CachingDevice`]: crate::CachingDevice
#[cfg(unix)]
fn measure_size(file: &File) -> Result<u64> {
    use std::os::unix::fs::FileTypeExt;
    let meta = file.metadata()?;
    let ft = meta.file_type();
    if !ft.is_block_device() && !ft.is_char_device() {
        return Ok(meta.len());
    }
    device_size_bytes(file)
}

/// Windows keeps the metadata length, because no equivalent measurement
/// has been made there. `\\.\PhysicalDriveN` is therefore still subject
/// to the defect this function exists to fix; saying so is better than
/// shipping an untested `DeviceIoControl` and implying otherwise.
#[cfg(not(unix))]
fn measure_size(file: &File) -> Result<u64> {
    Ok(file.metadata()?.len())
}

// The crate has no dependencies and this is not worth acquiring one for:
// `ioctl` is in libc, which is already linked into every std target.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux"))]
unsafe extern "C" {
    fn ioctl(fd: std::os::raw::c_int, request: std::os::raw::c_ulong, ...) -> std::os::raw::c_int;
}

/// macOS: `<sys/disk.h>` gives the block size and the block count
/// separately, and neither alone is the answer.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn device_size_bytes(file: &File) -> Result<u64> {
    use std::io;
    use std::os::fd::AsRawFd;
    // _IOR('d', 24, u32) and _IOR('d', 25, u64).
    const DKIOCGETBLOCKSIZE: std::os::raw::c_ulong = 0x4004_6418;
    const DKIOCGETBLOCKCOUNT: std::os::raw::c_ulong = 0x4008_6419;

    let fd = file.as_raw_fd();
    let mut block_size: u32 = 0;
    let mut block_count: u64 = 0;
    // SAFETY: `fd` is open for as long as `file` is borrowed, and each
    // request writes exactly the width its own encoding names into a
    // local of precisely that type.
    unsafe {
        if ioctl(fd, DKIOCGETBLOCKSIZE, &raw mut block_size) < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if ioctl(fd, DKIOCGETBLOCKCOUNT, &raw mut block_count) < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    block_count
        .checked_mul(u64::from(block_size))
        .ok_or_else(|| {
            Error::Io(io::Error::other(format!(
                "device reports {block_count} blocks of {block_size} bytes, \
                 whose product does not fit in u64"
            )))
        })
}

/// Linux: `<linux/fs.h>` answers in bytes in one call.
#[cfg(target_os = "linux")]
fn device_size_bytes(file: &File) -> Result<u64> {
    use std::io;
    use std::os::fd::AsRawFd;
    // _IOR(0x12, 114, size_t).
    const BLKGETSIZE64: std::os::raw::c_ulong = 0x8008_1272;

    let mut size: u64 = 0;
    // SAFETY: as above -- one call, writing one u64 into a u64.
    let rc = unsafe { ioctl(file.as_raw_fd(), BLKGETSIZE64, &raw mut size) };
    if rc < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(size)
}

/// Every other Unix: refuse rather than guess. `lseek` is measured wrong
/// on one of the two platforms tested, so extending it here on the
/// strength of that would be picking the silent failure.
#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "linux"))
))]
fn device_size_bytes(_file: &File) -> Result<u64> {
    use std::io;
    Err(Error::Io(io::Error::other(
        "no measured way to read a device node's size on this platform; \
         open the backing image file rather than the device node",
    )))
}

impl FileDevice {
    /// The guard a read holds for the whole of `read_at`.
    ///
    /// Unix takes it SHARED: `pread` carries its own offset, so readers
    /// do not disturb each other and only need to be kept apart from
    /// writers.
    ///
    /// Windows takes it EXCLUSIVE, because `seek_read` moves the file
    /// pointer — there, one reader really can spoil another's offset, so
    /// readers must exclude readers as well as writers. Same lock, same
    /// call site, and only that platform pays for the cursor.
    #[cfg(unix)]
    fn read_guard(&self) -> std::sync::RwLockReadGuard<'_, ()> {
        self.io_lock.read().unwrap()
    }

    #[cfg(windows)]
    fn read_guard(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.io_lock.write().unwrap()
    }

    /// One positioned read, returning what it got.
    ///
    /// TAKES NO LOCK ON EITHER PLATFORM. The caller holds `read_guard`
    /// for the whole read; acquiring anything here would be a second,
    /// non-reentrant acquisition of the same lock — on Windows, where
    /// that guard is exclusive, an immediate self-deadlock.
    #[cfg(unix)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::unix::fs::FileExt;
        Ok(self.file.read_at(buf, offset)?)
    }

    /// Windows: `seek_read` DOES move the file pointer. The exclusion
    /// that needs is held by the caller's guard, not taken here — see
    /// `read_guard`.
    #[cfg(windows)]
    fn read_once(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        use std::os::windows::fs::FileExt;
        Ok(self.file.seek_read(buf, offset)?)
    }
}

impl BlockRead for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        // HELD ACROSS THE WHOLE LOOP, NOT AROUND EACH POSITIONED READ.
        //
        // The loop below can issue several reads for one call, and a
        // guard taken inside it would let a `write_at` land between two
        // of them: the caller would get some bytes from before the write
        // and some from after, which is the tear this exists to prevent
        // moved one level down and made rarer. Rarer is worse — nobody
        // can reproduce it.
        //
        // NO TEST PINS THIS PLACEMENT, and the reason is worth writing
        // down because the obvious one is wrong. A regular file does
        // return short reads — at EOF — and this loop retries rather
        // than failing on one, so a read spanning EOF really does run
        // twice and the guard really would be released in between. That
        // discriminator was built: a 4096-byte file, a reader asking
        // 4090..4102, a writer parked on the lock growing the file at
        // 4090, where interleaved old-and-new bytes are reachable no
        // other way. 200 trials with the guard moved inside the loop
        // produced 0 tears. The window between releasing at the end of
        // one iteration and retaking at the start of the next is a few
        // instructions, and a writer already waiting never won it.
        //
        // So this is unwitnessed, not inert: the placement is correct
        // and the race it prevents is simply too narrow to enter on
        // demand. Measured, not argued.
        let _guard = self.read_guard();

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
        // EXCLUSIVE: excludes other writers' seeks and every reader.
        let _guard = self.io_lock.write().unwrap();
        let mut f = &self.file;
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        // EXCLUSIVE for the same reason as `write_at`: this pushes
        // buffered bytes at the file and must not interleave with a
        // write or a read.
        let _guard = self.io_lock.write().unwrap();
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

    use std::sync::mpsc;
    use std::time::Duration;

    /// How long an operation that must be blocked is given to prove it.
    /// Only a false PASS can come of this being short, and the
    /// ready-signal in each test removes the way that happens.
    const BLOCKED_FOR: Duration = Duration::from_millis(300);
    /// How long an operation that must finish is given. Generous on
    /// purpose: a loaded machine makes it slower, not flakier.
    const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

    /// A 4 KiB image of one repeated byte, opened read-write.
    fn rw_image(tag: &str, fill: u8) -> (std::sync::Arc<FileDevice>, Cleanup) {
        let path = temp_path(tag);
        let cleanup = Cleanup(path.clone());
        std::fs::write(&path, vec![fill; 4096]).expect("write the image");
        let dev = std::sync::Arc::new(FileDevice::open_rw(&path).expect("open rw"));
        (dev, cleanup)
    }

    /// A READ CONCURRENT WITH A WRITE MUST NOT PROCEED.
    ///
    /// This is the regression. Reads were briefly taken with no lock at
    /// all, which quietly removed the exclusion the original single
    /// mutex gave and left a read free to run through the middle of a
    /// `write_at` — `write_all` may become several `write` calls, and
    /// `read_at` is itself a loop, so either side can split.
    ///
    /// # Why the lock rather than the tear is the assertion
    ///
    /// The obvious test races a writer against readers and looks for a
    /// region holding bytes from both sides of the write. On a regular
    /// file that test cannot fail: a single `write` call is atomic
    /// against `pread` on both Linux and macOS, and `write_all` only
    /// splits above roughly 2 GiB, so the tear it looks for is
    /// unreachable at any size a test would use. It would pass with the
    /// fix reverted — an assertion whose outcome does not depend on the
    /// defect, which is worse than no assertion.
    ///
    /// So the exclusion itself is asserted, by holding the very guard
    /// `write_at` takes and requiring that a read cannot get past it.
    /// That is deterministic, needs no tear to be reproducible, and
    /// fails the moment `read_at` stops taking the lock.
    #[test]
    fn a_read_cannot_proceed_while_a_write_holds_the_lock() {
        let (dev, _c) = rw_image("read_excluded_by_write", 0x5A);

        // Stands in for a write in progress: the same exclusive guard
        // `write_at` holds across its seek and write.
        let held = dev.io_lock.write().unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = ready_tx.send(());
                let outcome = dev.read_at(0, &mut buf);
                let _ = done_tx.send(outcome.map(|()| buf[0]));
            });
        }

        // PROVE THE READER REACHED THE CALL. A thread that has not been
        // scheduled yet looks exactly like a thread correctly blocked,
        // and without this the assertion below would pass for that
        // reason on a busy machine.
        ready_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the reader thread never started");

        assert!(
            done_rx.recv_timeout(BLOCKED_FOR).is_err(),
            "a read completed while the exclusive write guard was held. Reads and \
             writes are not mutually excluded, so a read overlapping a write_at \
             can observe a partially written region"
        );

        // And it is blocked rather than broken: it completes once the
        // writer lets go. Without this half the test would pass against
        // a read_at that simply never returned.
        drop(held);
        let first = done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the read must proceed once the write guard is released")
            .expect("and must succeed");
        assert_eq!(first, 0x5A, "the read returned the wrong bytes");
    }

    /// AND THE EXCLUSION HOLDS THE OTHER WAY ROUND.
    ///
    /// A write must not start while a read is in progress, or the read
    /// it interleaves with is the one that tears. Asserted with a shared
    /// guard, which is what a Unix reader holds.
    #[test]
    fn a_write_cannot_proceed_while_a_read_holds_the_lock() {
        let (dev, _c) = rw_image("write_excluded_by_read", 0x11);

        // Stands in for a read in progress.
        let held = dev.io_lock.read().unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let _ = ready_tx.send(());
                let _ = done_tx.send(dev.write_at(0, &[0x22u8; 4096]));
            });
        }
        ready_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the writer thread never started");

        assert!(
            done_rx.recv_timeout(BLOCKED_FOR).is_err(),
            "a write completed while a read guard was held; a write_at may not \
             run through a read that is already in progress"
        );

        drop(held);
        done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the write must proceed once the read releases")
            .expect("and must succeed");

        let mut buf = [0u8; 4];
        dev.read_at(0, &mut buf).expect("read back");
        assert_eq!(buf, [0x22; 4], "the write did not land");
    }

    /// AND A FLUSH IS A WRITE FOR THIS PURPOSE.
    ///
    /// `flush` pushes buffered bytes at the file and calls `sync_data`,
    /// so it must not interleave with a read or a write any more than
    /// `write_at` may. The exclusive guard was here before this test
    /// was, and stating an invariant in a comment is not testing it:
    /// with the guard removed the whole suite stayed green.
    #[test]
    fn a_flush_cannot_proceed_while_a_read_holds_the_lock() {
        let (dev, _c) = rw_image("flush_excluded_by_read", 0x33);

        // Stands in for a read in progress.
        let held = dev.io_lock.read().unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let _ = ready_tx.send(());
                let _ = done_tx.send(dev.flush());
            });
        }
        ready_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the flushing thread never started");

        assert!(
            done_rx.recv_timeout(BLOCKED_FOR).is_err(),
            "a flush completed while a read guard was held; flush takes the lock \
             exclusively for the same reason write_at does"
        );

        drop(held);
        done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the flush must proceed once the read releases")
            .expect("and must succeed");
    }

    /// READERS STILL OVERLAP, WHICH IS THE POINT OF THE SHARED GUARD.
    ///
    /// THE OVER-CORRECTION THIS CATCHES: restoring read/write exclusion
    /// with a plain mutex, or by taking the write half of this lock on
    /// the read path, would pass both tests above and quietly undo the
    /// reader parallelism the positioned-read work existed for. Nothing
    /// else in the suite would notice, because every other assertion is
    /// about bytes and serialised readers return the right bytes.
    ///
    /// Unix only: on Windows `seek_read` moves the file pointer, so
    /// readers there take the guard exclusively on purpose and this
    /// would correctly block.
    #[test]
    #[cfg(unix)]
    fn a_read_does_not_exclude_another_read() {
        let (dev, _c) = rw_image("reads_overlap", 0x77);

        // Stands in for another reader already inside `read_at`.
        let held = dev.io_lock.read().unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        {
            let dev = std::sync::Arc::clone(&dev);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let _ = ready_tx.send(());
                let outcome = dev.read_at(0, &mut buf);
                let _ = done_tx.send(outcome.map(|()| buf[0]));
            });
        }
        ready_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect("the reader thread never started");

        let first = done_rx
            .recv_timeout(UNBLOCKED_WITHIN)
            .expect(
                "a read blocked behind another read. On Unix the guard must be \
                 shared -- positioned reads need no cursor, and serialising them \
                 undoes the parallelism the read path was rewritten for",
            )
            .expect("and the read must succeed");
        assert_eq!(first, 0x77);
        drop(held);
    }
}
