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
    ///
    /// # It must not change for the life of the device
    ///
    /// Callers are entitled to read it once, act on it, and read it again
    /// later expecting the same answer — [`crate::CachingDevice`] does
    /// exactly that, bounding a read against it and then clamping each
    /// block it fetches against it. A device whose size moves between the
    /// two makes the cache hold a block shorter than the read it was
    /// fetched for.
    ///
    /// An implementation whose length genuinely changes — a file being
    /// appended to, a volume being grown — should be reopened rather than
    /// reporting a new number through the same handle.
    ///
    /// # What the implementations here actually do
    ///
    /// This used to say "every implementation in this crate takes its size
    /// once, at construction". That was not true of any of the three
    /// shapes below, and it is the sentence an implementer in a sibling
    /// crate reads before deciding their own device may report a live
    /// length. [`crate::caching_device`]'s resizing path and
    /// `tests/caching_size_change.rs` both used to describe this
    /// differently again, so the crate said three incompatible things
    /// about one contract.
    ///
    /// - [`crate::FileDevice`] and the slice devices in
    ///   [`crate::slice`] do take their size once, at construction.
    /// - [`crate::ReadOnlyDevice`], [`crate::CountingDevice`] and
    ///   [`crate::CachingDevice`] FORWARD the question to the device they
    ///   wrap, on every call, and so are exactly as stable as it is. They
    ///   cannot be more: a wrapper has no way to hold a moving device
    ///   still. The `Arc<T>`, `Box<T>` and `&T` blanket impls below
    ///   forward the same way.
    /// - [`crate::CallbackDevice`] keeps its size in a PUBLIC field, so
    ///   nothing stops a caller moving it after construction. Stability
    ///   there is the caller's to keep, not the type's to enforce.
    ///
    /// So the contract binds the implementer; it is not something this
    /// crate's types guarantee on their behalf. `tests/
    /// size_stability_contract.rs` tests each of these behaviours rather
    /// than leaving this paragraph to be believed.
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

impl<T: BlockRead + ?Sized> BlockRead for &T {
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
