//! Block-device traits.
//!
//! Two layers because not every consumer needs writes:
//!
//! - [`BlockRead`] is the minimum: positioned reads + size. Disk-image
//!   readers (qcow2 reading) and probes (partition table walk) only need
//!   this much.
//! - [`BlockDevice`] extends `BlockRead` with optional `write_at` / `flush`
//!   / `is_writable`. Read-only devices that opt into the larger trait
//!   inherit the default `Err(ReadOnly)` write path automatically.
//!
//! Both traits are `Send + Sync` so callers can hold them behind `Arc<dyn _>`
//! across thread boundaries.

use crate::error::{Error, Result};

/// Read-only random-access block device.
pub trait BlockRead: Send + Sync {
    /// Read exactly `buf.len()` bytes starting at `offset` (bytes from the
    /// start of the device).
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Total device size in bytes. Used for bounds checks.
    fn size_bytes(&self) -> u64;
}

/// Read-write random-access block device. Implementors that genuinely
/// support writes override `write_at` / `flush` / `is_writable`. The
/// defaults model a strict read-only device.
pub trait BlockDevice: BlockRead {
    /// Write exactly `buf.len()` bytes at `offset`. Default: returns
    /// [`Error::ReadOnly`].
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> Result<()> {
        Err(Error::ReadOnly)
    }

    /// Flush pending writes to stable storage. No-op by default.
    fn flush(&self) -> Result<()> {
        Ok(())
    }

    /// Whether `write_at` is likely to succeed. Mount paths use this to
    /// decide whether to attempt journal replay or stay strict-read-only.
    fn is_writable(&self) -> bool {
        false
    }
}

// Forwarding impls so `Arc<T>` and `Box<T>` work transparently as
// `&dyn BlockRead` / `&dyn BlockDevice`.

impl<T: BlockRead + ?Sized> BlockRead for std::sync::Arc<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for std::sync::Arc<T> {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        (**self).write_at(offset, buf)
    }
    fn flush(&self) -> Result<()> {
        (**self).flush()
    }
    fn is_writable(&self) -> bool {
        (**self).is_writable()
    }
}

impl<T: BlockRead + ?Sized> BlockRead for Box<T> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }
}

impl<T: BlockDevice + ?Sized> BlockDevice for Box<T> {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        (**self).write_at(offset, buf)
    }
    fn flush(&self) -> Result<()> {
        (**self).flush()
    }
    fn is_writable(&self) -> bool {
        (**self).is_writable()
    }
}
