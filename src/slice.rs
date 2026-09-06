//! Slice adapters — view a byte sub-range of any `BlockRead` as its own
//! device. Useful any time you want to feed a fragment of a larger
//! device to a consumer that expects a whole block source — partition
//! probes, image-file extents, mmap-style views, fuzzer harnesses.
//!
//! Three variants:
//!
//! - [`SliceReader`] borrows the parent, lifetime-tied. Cheaper when the
//!   parent outlives the slice and you can express that statically.
//! - [`OwnedSlice`] holds an `Arc` to the parent. Use when the parent's
//!   lifetime can't be expressed in a borrow (FFI handles, slice handed
//!   across thread boundaries, etc.).
//! - [`OwnedRwSlice`] holds an `Arc<dyn BlockDevice>` and propagates
//!   writes to the parent.
//!
//! The first two are strictly read-only: the default `Err(ReadOnly)`
//! write path from [`BlockDevice`] applies.
//!
//! # Which error an out-of-range request gets
//!
//! All three share one range check — `SliceGeometry::rebase` — and it
//! answers in two different currencies depending on the direction of the
//! request:
//!
//! | request outside `[0, length)` | error |
//! |---|---|
//! | read  | [`Error::ShortRead`] with `got: 0` |
//! | write | [`Error::OutOfBounds`] |
//!
//! The asymmetry is deliberate. A slice exists to be substitutable for a
//! real device of size `length`, and a real device — [`FileDevice`] —
//! answers a read that begins at or past its end with exactly
//! `ShortRead { offset, want, got: 0 }`. A slice that answered
//! `OutOfBounds` would be distinguishable from the thing it stands in
//! for, and every caller that already handles end-of-device would need a
//! second arm to cope with slices. Writes have no partial-write variant
//! to stay consistent with, and a caller that overran a write needs the
//! device size in order to clamp and retry — which is what
//! [`Error::OutOfBounds`] carries and [`Error::ShortRead`] does not.
//!
//! The match is on the variant, not on `got`. A slice refuses an
//! out-of-range read before it touches the parent, so it reports `got: 0`
//! and leaves the buffer untouched — including for a read that begins
//! inside the slice and runs off its end, where [`FileDevice`] would have
//! copied the readable prefix and reported its length. `got` counts bytes
//! actually delivered, and a slice delivers none.
//!
//! This governs the slice's own range only. A request that *is* inside
//! `[0, length)` is forwarded to the parent, and whatever the parent says
//! about it — including [`Error::OutOfBounds`] from a container reader
//! that knows its virtual size — comes back unchanged.
//!
//! [`FileDevice`]: crate::FileDevice

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::sync::Arc;

/// Where a slice sits on its parent, and the one bounds rule the three
/// slice types share.
///
/// The public slice types differ only in how they hold the parent and
/// whether writes propagate. The geometry, the range check and the choice
/// of error are identical across all of them, so they live here — one
/// definition to read, one place to change.
#[derive(Clone, Copy)]
struct SliceGeometry {
    start: u64,
    length: u64,
}

impl SliceGeometry {
    fn new(start: u64, length: u64) -> Self {
        Self { start, length }
    }

    /// Parent offset corresponding to `offset`, or `None` when
    /// `[offset, offset + len)` is not wholly inside `[0, length)`, or
    /// when the rebased offset would not fit on the parent at all.
    ///
    /// Every one of these additions is checked, `start + offset`
    /// included. That one used to be deliberate, on the argument that a
    /// slice built with a nonsense `start` would "overflow here rather
    /// than quietly reading some other part of the parent" -- which
    /// holds only while `overflow-checks` is on, and it is off in the
    /// release profile these crates ship. In release the addition
    /// wrapped, and the wrap did precisely the thing the argument said
    /// it avoided: a slice starting at 2^63 and 5000 bytes long
    /// returned `Ok` and the parent's bytes from offset 5000.
    ///
    /// A slice's geometry comes from a partition table, which comes off
    /// the disk, so "a nonsense start" is an ordinary thing to be
    /// handed rather than a programming mistake.
    fn rebase(&self, offset: u64, len: u64) -> Option<u64> {
        let end = offset.checked_add(len)?;
        if end > self.length {
            return None;
        }
        self.start.checked_add(offset)
    }

    /// Bounds-check a read and rebase it onto the parent.
    ///
    /// Out of range is [`Error::ShortRead`] with `got: 0` — the same
    /// answer a real device of size `length` gives for a read beginning
    /// at or past its end. See the module docs for why.
    fn rebase_read(&self, offset: u64, len: usize) -> Result<u64> {
        self.rebase(offset, len as u64).ok_or(Error::ShortRead {
            offset,
            want: len,
            got: 0,
        })
    }

    /// Bounds-check a write and rebase it onto the parent.
    ///
    /// Out of range is [`Error::OutOfBounds`]: nothing was written, and
    /// the caller is handed the slice's size so it can clamp and retry.
    fn rebase_write(&self, offset: u64, len: usize) -> Result<u64> {
        self.rebase(offset, len as u64).ok_or(Error::OutOfBounds {
            offset,
            len: len as u64,
            size: self.length,
        })
    }
}

/// Borrowed slice of a parent `BlockRead`.
///
/// `read_at(0, …)` reads `start` of the parent. Reads outside
/// `[0, length)` return [`Error::ShortRead`] with `got: 0`.
pub struct SliceReader<'a> {
    parent: &'a (dyn BlockRead + 'a),
    geom: SliceGeometry,
}

impl<'a> SliceReader<'a> {
    pub fn new(parent: &'a (dyn BlockRead + 'a), start: u64, length: u64) -> Self {
        Self {
            parent,
            geom: SliceGeometry::new(start, length),
        }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl<'a> BlockRead for SliceReader<'a> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

/// Slices are read-only by default — even where the parent is writable,
/// slicing is almost always paired with a read-only inspection or
/// dispatch workflow.
impl<'a> BlockDevice for SliceReader<'a> {}

/// Owned slice over an `Arc<dyn BlockRead>`. Use when the parent's
/// lifetime can't be expressed in a borrow — e.g. when the slice is
/// handed across an FFI boundary or stored in a long-lived struct.
///
/// Reads outside `[0, length)` return [`Error::ShortRead`] with `got: 0`.
pub struct OwnedSlice {
    parent: Arc<dyn BlockRead>,
    geom: SliceGeometry,
}

impl OwnedSlice {
    pub fn new(parent: Arc<dyn BlockRead>, start: u64, length: u64) -> Self {
        Self {
            parent,
            geom: SliceGeometry::new(start, length),
        }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl BlockRead for OwnedSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

/// Same rationale as `SliceReader`: read-only by default.
impl BlockDevice for OwnedSlice {}

/// Owned, read-WRITE slice over an `Arc<dyn BlockDevice>`. Use when the
/// parent is writable and the slice should propagate writes (e.g. an
/// individual partition handed to a filesystem driver).
///
/// Reads outside `[0, length)` return [`Error::ShortRead`] with `got: 0`;
/// writes outside it return [`Error::OutOfBounds`]. The two directions
/// differ on purpose — see the module docs.
pub struct OwnedRwSlice {
    parent: Arc<dyn BlockDevice>,
    geom: SliceGeometry,
}

impl OwnedRwSlice {
    pub fn new(parent: Arc<dyn BlockDevice>, start: u64, length: u64) -> Self {
        Self {
            parent,
            geom: SliceGeometry::new(start, length),
        }
    }

    /// Byte offset of this slice on the parent device.
    pub fn start(&self) -> u64 {
        self.geom.start
    }

    /// Length of this slice in bytes (== `size_bytes()`).
    pub fn length(&self) -> u64 {
        self.geom.length
    }
}

impl BlockRead for OwnedRwSlice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let at = self.geom.rebase_read(offset, buf.len())?;
        self.parent.read_at(at, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.geom.length
    }
}

impl BlockDevice for OwnedRwSlice {
    /// Range first, writability second: a write that is both out of range
    /// and aimed at a read-only parent reports [`Error::OutOfBounds`],
    /// not [`Error::ReadOnly`]. The range is a property of this slice and
    /// is knowable without asking the parent anything, so it is the more
    /// specific of the two answers.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let at = self.geom.rebase_write(offset, buf.len())?;
        if !self.parent.is_writable() {
            return Err(Error::ReadOnly);
        }
        self.parent.write_at(at, buf)
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
    use crate::test_device::{Bytes, RwBytes};
    use std::sync::Mutex;

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

    /// A slice's geometry comes from a partition table, and a partition
    /// table comes off the disk. A start and a length that add up past
    /// 2^64 are an ordinary thing to be handed.
    ///
    /// In a release build the rebasing addition wrapped, so a read at an
    /// offset inside the slice's declared length landed somewhere else
    /// on the parent entirely -- and came back `Ok`, with those bytes,
    /// as though they were the slice's own.
    #[test]
    fn a_slice_whose_start_plus_offset_leaves_the_parent_reads_nothing() {
        let mut v = vec![0u8; 64 * 1024];
        v[5000..5008].copy_from_slice(b"SECRET!!");
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(v)));

        // A GPT entry of starting_lba = 2^54 and ending_lba = 2^55 + 99
        // produces exactly this.
        let slice = OwnedSlice::new(dev, 1 << 63, (1 << 63) + 51200);
        let mut buf = [0u8; 8];
        let inside_the_declared_length = (1u64 << 63) + 5000;

        let outcome = slice.read_at(inside_the_declared_length, &mut buf);
        assert!(
            outcome.is_err(),
            "the read succeeded and returned {:?}, which is the parent's \
             bytes from offset 5000",
            std::str::from_utf8(&buf)
        );
        assert_ne!(&buf, b"SECRET!!");
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

    #[test]
    fn owned_slice_accessors_report_geometry() {
        let dev: Arc<dyn BlockRead> = Arc::new(Bytes(Mutex::new(vec![0u8; 4096])));
        let slice = OwnedSlice::new(dev, 512, 256);
        assert_eq!(slice.start(), 512);
        assert_eq!(slice.length(), 256);
        assert_eq!(slice.size_bytes(), 256);
    }

    #[test]
    fn owned_rw_slice_accessors_report_geometry() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 16, 32);
        assert_eq!(slice.start(), 16);
        assert_eq!(slice.length(), 32);
        assert_eq!(slice.size_bytes(), 32);
        assert!(slice.is_writable());
    }

    #[test]
    fn owned_rw_slice_rebases_reads_and_writes() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev.clone(), 16, 32);

        // Write through the slice lands at parent offset 16.
        slice.write_at(0, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        let mut buf = [0u8; 4];
        slice.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0xDE, 0xAD, 0xBE, 0xEF]);

        // Confirm rebasing against the parent directly.
        let mut pbuf = [0u8; 4];
        dev.read_at(16, &mut pbuf).unwrap();
        assert_eq!(pbuf, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn owned_rw_slice_rejects_out_of_bounds_write() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 0, 8);
        match slice.write_at(6, &[0u8; 4]) {
            Err(Error::OutOfBounds { .. }) => {}
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    /// The bounds rule is direction-dependent by design: one slice, one
    /// out-of-range span, two different errors. Pinned here so the
    /// asymmetry cannot be "tidied up" into consistency without someone
    /// deciding to — the reasoning is in the module docs.
    #[test]
    fn same_out_of_range_span_is_short_read_for_a_read_and_out_of_bounds_for_a_write() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 64])));
        let slice = OwnedRwSlice::new(dev, 16, 8);

        let mut buf = [0u8; 4];
        match slice.read_at(6, &mut buf) {
            Err(Error::ShortRead { offset, want, got }) => {
                assert_eq!((offset, want, got), (6, 4, 0));
            }
            other => panic!("expected ShortRead, got {other:?}"),
        }

        match slice.write_at(6, &[0u8; 4]) {
            Err(Error::OutOfBounds { offset, len, size }) => {
                assert_eq!((offset, len, size), (6, 4, 8));
            }
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn owned_rw_slice_flush_delegates_to_parent() {
        let dev: Arc<dyn BlockDevice> = Arc::new(RwBytes(Mutex::new(vec![0u8; 8])));
        let slice = OwnedRwSlice::new(dev, 0, 8);
        // Default `flush` on RwBytes is a no-op success; the slice forwards it.
        slice.flush().unwrap();
    }
}
