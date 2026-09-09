//! ReadOnlyDevice wrapper — integration variants beyond the inline tests in
//! src/readonly.rs (notably the Arc<dyn> usage and flush behaviour).

use fs_core::{BlockDevice, BlockRead, Error, ReadOnlyDevice, Result};
use std::sync::{Arc, Mutex};

mod common;

struct WritableBytes {
    bytes: Mutex<Vec<u8>>,
    flushes: Mutex<u32>,
}
impl WritableBytes {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Mutex::new(bytes),
            flushes: Mutex::new(0),
        }
    }
}
impl BlockRead for WritableBytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.bytes.lock().unwrap();
        common::read_into(&b, offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        self.bytes.lock().unwrap().len() as u64
    }
}
impl BlockDevice for WritableBytes {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.bytes.lock().unwrap();
        common::write_from(&mut b, offset, buf)
    }
    fn flush(&self) -> Result<()> {
        *self.flushes.lock().unwrap() += 1;
        Ok(())
    }
    fn is_writable(&self) -> bool {
        true
    }
}

/// The double underneath survives a wild offset, which is the point of
/// routing it through `common`: a test that wants to prove the WRAPPER
/// refuses an offset cannot be written at all if the device beneath it
/// panics on that input first. This one goes through the wrapper.
#[test]
fn a_wild_offset_through_the_wrapper_is_refused_not_a_panic() {
    let inner: Arc<dyn BlockRead> = Arc::new(WritableBytes::new(vec![1, 2, 3, 4, 5, 6, 7, 8]));
    let wrapped = ReadOnlyDevice::new(inner);
    let mut buf = [0u8; 8];
    match wrapped
        .read_at(u64::MAX, &mut buf)
        .expect_err("past the end")
    {
        Error::ShortRead { offset, want, got } => {
            assert_eq!((offset, want, got), (u64::MAX, 8, 0))
        }
        other => panic!("expected ShortRead, got {other:?}"),
    }
    assert_eq!(buf, [0u8; 8], "a refused read leaves the buffer alone");
}

#[test]
fn wrapping_arc_dyn_blockread_works() {
    let inner: Arc<dyn BlockRead> = Arc::new(WritableBytes::new(vec![1, 2, 3, 4, 5, 6, 7, 8]));
    let wrapped = ReadOnlyDevice::new(inner);
    assert_eq!(wrapped.size_bytes(), 8);
    let mut buf = [0u8; 4];
    wrapped.read_at(2, &mut buf).unwrap();
    assert_eq!(buf, [3, 4, 5, 6]);
    assert!(!BlockDevice::is_writable(&wrapped));
}

#[test]
fn wrapping_arc_dyn_blockdevice_hides_writability() {
    // Parent IS writable, but the wrapper must report otherwise.
    let parent_inner = Arc::new(WritableBytes::new(vec![0u8; 16]));
    let parent: Arc<dyn BlockDevice> = parent_inner.clone();
    let wrapped = ReadOnlyDevice::new(parent);

    assert!(!BlockDevice::is_writable(&wrapped));
    let err = BlockDevice::write_at(&wrapped, 0, &[1u8; 4]).unwrap_err();
    assert!(matches!(err, Error::ReadOnly));

    // Underlying device was never mutated by the wrapper.
    let bytes = parent_inner.bytes.lock().unwrap();
    assert_eq!(*bytes, vec![0u8; 16]);
}

#[test]
fn flush_default_impl_is_noop_ok() {
    let wrapped = ReadOnlyDevice::new(WritableBytes::new(vec![0u8; 4]));
    // Default BlockDevice::flush is Ok(()); wrapper does not delegate.
    BlockDevice::flush(&wrapped).unwrap();
}

#[test]
fn inner_borrow_does_not_consume_wrapper() {
    let wrapped = ReadOnlyDevice::new(WritableBytes::new(vec![9, 8, 7, 6]));
    {
        let inner_ref = wrapped.inner();
        assert_eq!(inner_ref.size_bytes(), 4);
    }
    // Wrapper still usable.
    let mut buf = [0u8; 2];
    wrapped.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, [9, 8]);
}

#[test]
fn into_inner_round_trips_and_recovers_writability() {
    let wrapped = ReadOnlyDevice::new(WritableBytes::new(vec![0u8; 4]));
    let inner = wrapped.into_inner();
    assert!(inner.is_writable());
    inner.write_at(0, &[1, 2, 3, 4]).unwrap();
    let mut buf = [0u8; 4];
    inner.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4]);
}

#[test]
fn read_only_wrapper_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ReadOnlyDevice<WritableBytes>>();
    assert_send_sync::<ReadOnlyDevice<Arc<dyn BlockDevice>>>();
}
