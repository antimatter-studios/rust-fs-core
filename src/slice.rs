//! Slice adapters — view a byte sub-range of any `BlockRead` as its own
//! device. Useful any time you want to feed a fragment of a larger
//! device to a consumer that expects a whole block source — partition
//! probes, image-file extents, mmap-style views, fuzzer harnesses.
//!
//! Two variants:
//!
//! - [`SliceReader`] borrows the parent, lifetime-tied. Cheaper when the
//!   parent outlives the slice and you can express that statically.
//! - [`OwnedSlice`] holds an `Arc` to the parent. Use when the parent's
//!   lifetime can't be expressed in a borrow (FFI handles, slice handed
//!   across thread boundaries, etc.).
//!
//! Both treat the slice as **read-only** — reads inside the range are
//! forwarded to the parent, reads outside return [`Error::ShortRead`].
//! The default `Err(ReadOnly)` write path from [`BlockDevice`] applies.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::sync::Arc;

/// Borrowed slice of a parent `BlockRead`.
///
/// `read_at(0, …)` reads `start` of the parent. Reads past `length`
/// return [`Error::ShortRead`].
pub struct SliceReader<'a> {
    parent: &'a (dyn BlockRead + 'a),
    start: u64,
    length: u64,
}

impl<'a> SliceReader<'a> {
    pub fn new(parent: &'a (dyn BlockRead + 'a), start: u64, length: u64) -> Self {
        Self {
            parent,
            start,
            length,
        }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.length
    }
}

impl<'a> BlockRead for SliceReader<'a> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let want = buf.len() as u64;
        if offset
            .checked_add(want)
            .map(|e| e > self.length)
            .unwrap_or(true)
        {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: 0,
            });
        }
        self.parent.read_at(self.start + offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.length
    }
}

/// Slices are read-only by default — even where the parent is writable,
/// slicing is almost always paired with a read-only inspection or
/// dispatch workflow.
impl<'a> BlockDevice for SliceReader<'a> {}

/// Owned slice over an `Arc<dyn BlockRead>`. Use when the parent's
/// lifetime can't be expressed in a borrow — e.g. when the slice is
/// handed across an FFI boundary or stored in a long-lived struct.
pub struct OwnedSlice {
    parent: Arc<dyn BlockRead>,
    start: u64,
    length: u64,
}

impl OwnedSlice {
    pub fn new(parent: Arc<dyn BlockRead>, start: u64, length: u64) -> Self {
        Self {
            parent,
            start,
            length,
        }
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn length(&self) -> u64 {
        self.length
    }
}

impl BlockRead for OwnedSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let want = buf.len() as u64;
        if offset
            .checked_add(want)
            .map(|e| e > self.length)
            .unwrap_or(true)
        {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: 0,
            });
        }
        self.parent.read_at(self.start + offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.length
    }
}

/// Same rationale as `SliceReader`: read-only by default.
impl BlockDevice for OwnedSlice {}

/// Owned, read-WRITE slice over an `Arc<dyn BlockDevice>`. Use when the
/// parent is writable and the slice should propagate writes (e.g. an
/// individual partition handed to a filesystem driver). Reads + writes
/// outside `[0, length)` return [`Error::ShortRead`] / [`Error::OutOfBounds`].
pub struct OwnedRwSlice {
    parent: Arc<dyn BlockDevice>,
    start: u64,
    length: u64,
}

impl OwnedRwSlice {
    pub fn new(parent: Arc<dyn BlockDevice>, start: u64, length: u64) -> Self {
        Self {
            parent,
            start,
            length,
        }
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn length(&self) -> u64 {
        self.length
    }
}

impl BlockRead for OwnedRwSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let want = buf.len() as u64;
        if offset
            .checked_add(want)
            .map(|e| e > self.length)
            .unwrap_or(true)
        {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: 0,
            });
        }
        self.parent.read_at(self.start + offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.length
    }
}

impl BlockDevice for OwnedRwSlice {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let want = buf.len() as u64;
        if offset
            .checked_add(want)
            .map(|e| e > self.length)
            .unwrap_or(true)
        {
            return Err(Error::OutOfBounds {
                offset,
                len: want,
                size: self.length,
            });
        }
        if !self.parent.is_writable() {
            return Err(Error::ReadOnly);
        }
        self.parent.write_at(self.start + offset, buf)
    }

    fn flush(&self) -> Result<()> {
        self.parent.flush()
    }

    fn is_writable(&self) -> bool {
        self.parent.is_writable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Bytes(Mutex<Vec<u8>>);
    impl BlockRead for Bytes {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let b = self.0.lock().unwrap();
            let start = offset as usize;
            let end = start + buf.len();
            if end > b.len() {
                return Err(Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: b.len().saturating_sub(start),
                });
            }
            buf.copy_from_slice(&b[start..end]);
            Ok(())
        }
        fn size_bytes(&self) -> u64 {
            self.0.lock().unwrap().len() as u64
        }
    }

    #[test]
    fn slice_reader_rebases_offsets() {
        let mut v = vec![0u8; 4096];
        v[2000..2004].copy_from_slice(&[0xAB, 0xCD, 0xEF, 0x01]);
        let dev = Bytes(Mutex::new(v));

        let slice = SliceReader::new(&dev, 2000, 4);
        assert_eq!(slice.size_bytes(), 4);
        assert_eq!(slice.start(), 2000);
        assert_eq!(slice.length(), 4);

        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xAB, 0xCD, 0xEF, 0x01]);
    }

    #[test]
    fn slice_reader_rejects_out_of_bounds() {
        let dev = Bytes(Mutex::new(vec![0u8; 4096]));
        let slice = SliceReader::new(&dev, 0, 16);
        let mut buf = [0u8; 8];
        match slice.read_at(12, &mut buf) {
            Err(Error::ShortRead { .. }) => {}
            other => panic!("expected ShortRead, got {other:?}"),
        }
    }

    #[test]
    fn owned_slice_works_through_arc() {
        let mut v = vec![0u8; 4096];
        v[100..104].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(v)));

        let slice = OwnedSlice::new(dev, 100, 4);
        assert_eq!(slice.size_bytes(), 4);
        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn slices_reject_writes_via_blockdevice_default() {
        let dev = Bytes(Mutex::new(vec![0u8; 16]));
        let slice = SliceReader::new(&dev, 0, 8);
        let err = BlockDevice::write_at(&slice, 0, &[1u8; 4]).unwrap_err();
        assert!(matches!(err, Error::ReadOnly));
    }
}
