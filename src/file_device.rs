//! File-backed `BlockDevice`. Used for disk images, raw `/dev/diskN` reads,
//! anything that std::fs::File can address.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

pub struct FileDevice {
    file: Mutex<File>,
    size: u64,
    writable: bool,
}

impl FileDevice {
    /// Open read-only.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
            writable: false,
        })
    }

    /// Open read-write. Errors if the path is not writable.
    pub fn open_rw<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
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

impl BlockRead for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        let mut total = 0usize;
        while total < buf.len() {
            match f.read(&mut buf[total..])? {
                0 => {
                    return Err(Error::ShortRead {
                        offset,
                        want: buf.len(),
                        got: total,
                    });
                }
                n => total += n,
            }
        }
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.size
    }
}

impl BlockDevice for FileDevice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        let mut f = self.file.lock().unwrap();
        f.flush()?;
        f.sync_data()?;
        Ok(())
    }

    fn is_writable(&self) -> bool {
        self.writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
