//! OwnedRwSlice: writes, OOB rejection, ro-parent rejection, flush
//! propagation, plus slice-of-slice composition.

use fs_core::{BlockDevice, BlockRead, Error, OwnedRwSlice, OwnedSlice, Result, SliceReader};
use std::sync::{Arc, Mutex};

struct Bytes {
    storage: Mutex<Vec<u8>>,
    flushes: Mutex<u32>,
    writable: bool,
}
impl Bytes {
    fn new(bytes: Vec<u8>, writable: bool) -> Self {
        Self {
            storage: Mutex::new(bytes),
            flushes: Mutex::new(0),
            writable,
        }
    }
}
impl BlockRead for Bytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.storage.lock().unwrap();
        let s = offset as usize;
        let end = s + buf.len();
        if end > b.len() {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: b.len().saturating_sub(s),
            });
        }
        buf.copy_from_slice(&b[s..end]);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.storage.lock().unwrap().len() as u64
    }
}
impl BlockDevice for Bytes {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        let mut b = self.storage.lock().unwrap();
        let s = offset as usize;
        b[s..s + buf.len()].copy_from_slice(buf);
        Ok(())
    }
    fn flush(&self) -> Result<()> {
        *self.flushes.lock().unwrap() += 1;
        Ok(())
    }
    fn is_writable(&self) -> bool {
        self.writable
    }
}

#[test]
fn owned_rw_slice_writes_land_on_parent_at_offset() {
    let parent_inner = Arc::new(Bytes::new(vec![0u8; 32], true));
    let parent: Arc<dyn BlockDevice> = parent_inner.clone();
    let slice = OwnedRwSlice::new(parent, 8, 16);

    assert_eq!(slice.size_bytes(), 16);
    assert_eq!(slice.start(), 8);
    assert_eq!(slice.length(), 16);
    assert!(slice.is_writable());

    slice.write_at(4, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();

    // Read back through the slice.
    let mut buf = [0u8; 4];
    slice.read_at(4, &mut buf).unwrap();
    assert_eq!(buf, [0xAA, 0xBB, 0xCC, 0xDD]);

    // Verify it actually landed at parent offset 12 (slice.start=8 + offset=4).
    let raw = parent_inner.storage.lock().unwrap();
    assert_eq!(&raw[12..16], &[0xAA, 0xBB, 0xCC, 0xDD]);
}

#[test]
fn owned_rw_slice_write_past_length_returns_out_of_bounds() {
    let parent: Arc<dyn BlockDevice> = Arc::new(Bytes::new(vec![0u8; 32], true));
    let slice = OwnedRwSlice::new(parent, 0, 8);
    let err = slice.write_at(6, &[1, 2, 3, 4]).unwrap_err();
    match err {
        Error::OutOfBounds { offset, len, size } => {
            assert_eq!(offset, 6);
            assert_eq!(len, 4);
            assert_eq!(size, 8);
        }
        other => panic!("expected OutOfBounds, got {other:?}"),
    }
}

#[test]
fn owned_rw_slice_write_through_readonly_parent_returns_readonly() {
    let parent: Arc<dyn BlockDevice> = Arc::new(Bytes::new(vec![0u8; 32], false));
    let slice = OwnedRwSlice::new(parent, 0, 16);
    assert!(!slice.is_writable());
    let err = slice.write_at(0, &[1, 2, 3, 4]).unwrap_err();
    assert!(matches!(err, Error::ReadOnly));
}

#[test]
fn owned_rw_slice_flush_propagates_to_parent() {
    let parent_inner = Arc::new(Bytes::new(vec![0u8; 16], true));
    let parent: Arc<dyn BlockDevice> = parent_inner.clone();
    let slice = OwnedRwSlice::new(parent, 0, 16);
    slice.flush().unwrap();
    slice.flush().unwrap();
    assert_eq!(*parent_inner.flushes.lock().unwrap(), 2);
}

#[test]
fn owned_rw_slice_read_past_length_returns_short_read() {
    let parent: Arc<dyn BlockDevice> = Arc::new(Bytes::new(vec![0u8; 32], true));
    let slice = OwnedRwSlice::new(parent, 0, 8);
    let mut buf = [0u8; 4];
    match slice.read_at(6, &mut buf) {
        Err(Error::ShortRead { offset, want, got }) => {
            assert_eq!(offset, 6);
            assert_eq!(want, 4);
            assert_eq!(got, 0);
        }
        other => panic!("expected ShortRead, got {other:?}"),
    }
}

#[test]
fn slice_reader_nested_in_owned_slice_rebases_correctly() {
    // Parent has bytes [0..32). OwnedSlice = [8..24) of parent. SliceReader
    // = [4..12) of that OwnedSlice. Should yield parent bytes [12..20).
    let mut v = vec![0u8; 32];
    for (i, b) in v.iter_mut().enumerate() {
        *b = i as u8;
    }
    let parent: Arc<dyn BlockRead> = Arc::new(Bytes::new(v, false));
    let outer = OwnedSlice::new(parent, 8, 16);
    assert_eq!(outer.size_bytes(), 16);

    let inner = SliceReader::new(&outer, 4, 8);
    assert_eq!(inner.size_bytes(), 8);
    let mut buf = [0u8; 8];
    inner.read_at(0, &mut buf).unwrap();
    // parent[12..20] = 12..20.
    assert_eq!(buf, [12, 13, 14, 15, 16, 17, 18, 19]);
}

#[test]
fn owned_rw_slice_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OwnedRwSlice>();
}

#[test]
fn owned_rw_slice_write_offset_overflow_is_out_of_bounds() {
    let parent: Arc<dyn BlockDevice> = Arc::new(Bytes::new(vec![0u8; 32], true));
    let slice = OwnedRwSlice::new(parent, 0, 8);
    let err = slice.write_at(u64::MAX - 1, &[1, 2, 3, 4]).unwrap_err();
    assert!(matches!(err, Error::OutOfBounds { .. }));
}
