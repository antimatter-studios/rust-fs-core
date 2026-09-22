//! Forwarding impls of BlockRead / BlockDevice for Arc<T>, Box<T>, &T.

use fs_core::{BlockDevice, BlockRead, Error, Result};
use std::sync::{Arc, Mutex};

mod common;

struct Tracker {
    bytes: Mutex<Vec<u8>>,
    reads: Mutex<u64>,
    writes: Mutex<u64>,
    flushes: Mutex<u64>,
    /// Counts `set_len` calls that REACHED this device. A wrapper that
    /// answered the trait default instead of forwarding leaves it at 0
    /// while the call still returns something plausible.
    grows: Mutex<u64>,
    writable: bool,
}
impl Tracker {
    fn new(bytes: Vec<u8>, writable: bool) -> Self {
        Self {
            bytes: Mutex::new(bytes),
            reads: Mutex::new(0),
            writes: Mutex::new(0),
            flushes: Mutex::new(0),
            grows: Mutex::new(0),
            writable,
        }
    }
}
impl BlockRead for Tracker {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        *self.reads.lock().unwrap() += 1;
        let b = self.bytes.lock().unwrap();
        common::read_into(&b, offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        self.bytes.lock().unwrap().len() as u64
    }
}
impl BlockDevice for Tracker {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        *self.writes.lock().unwrap() += 1;
        let mut b = self.bytes.lock().unwrap();
        common::write_from(&mut b, offset, buf)
    }
    fn flush(&self) -> Result<()> {
        *self.flushes.lock().unwrap() += 1;
        Ok(())
    }
    fn is_writable(&self) -> bool {
        self.writable
    }
    fn set_len(&self, new_len: u64) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        *self.grows.lock().unwrap() += 1;
        self.bytes.lock().unwrap().resize(
            usize::try_from(new_len).expect("a test length that fits"),
            0,
        );
        Ok(())
    }
    fn can_grow(&self) -> bool {
        self.writable
    }
}

#[test]
fn arc_blockread_forwards_read_and_size() {
    let inner = Arc::new(Tracker::new(vec![0x11, 0x22, 0x33, 0x44], false));
    let dev: Arc<dyn BlockRead> = inner.clone();
    let mut buf = [0u8; 2];
    dev.read_at(1, &mut buf).unwrap();
    assert_eq!(buf, [0x22, 0x33]);
    assert_eq!(dev.size_bytes(), 4);
    assert_eq!(*inner.reads.lock().unwrap(), 1);
}

#[test]
fn arc_blockdevice_forwards_write_flush_is_writable() {
    let inner = Arc::new(Tracker::new(vec![0u8; 8], true));
    let dev: Arc<dyn BlockDevice> = inner.clone();
    assert!(dev.is_writable());
    dev.write_at(2, &[0xAA, 0xBB]).unwrap();
    dev.flush().unwrap();
    assert_eq!(*inner.writes.lock().unwrap(), 1);
    assert_eq!(*inner.flushes.lock().unwrap(), 1);
    let mut buf = [0u8; 2];
    dev.read_at(2, &mut buf).unwrap();
    assert_eq!(buf, [0xAA, 0xBB]);
}

#[test]
fn box_blockread_forwards() {
    let dev: Box<dyn BlockRead> = Box::new(Tracker::new(vec![1, 2, 3, 4, 5], false));
    assert_eq!(dev.size_bytes(), 5);
    let mut buf = [0u8; 3];
    dev.read_at(1, &mut buf).unwrap();
    assert_eq!(buf, [2, 3, 4]);
}

#[test]
fn box_blockdevice_forwards_write() {
    let dev: Box<dyn BlockDevice> = Box::new(Tracker::new(vec![0u8; 4], true));
    assert!(dev.is_writable());
    dev.write_at(0, &[9, 8, 7, 6]).unwrap();
    let mut buf = [0u8; 4];
    dev.read_at(0, &mut buf).unwrap();
    assert_eq!(buf, [9, 8, 7, 6]);
}

#[test]
fn box_blockdevice_forwards_readonly_rejection() {
    let dev: Box<dyn BlockDevice> = Box::new(Tracker::new(vec![0u8; 4], false));
    assert!(!dev.is_writable());
    match dev.write_at(0, &[1]) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected ReadOnly, got {other:?}"),
    }
}

#[test]
fn ref_blockread_forwards() {
    let owned = Tracker::new(vec![10, 20, 30, 40], false);
    let r: &dyn BlockRead = &owned;
    assert_eq!(r.size_bytes(), 4);
    let mut buf = [0u8; 2];
    r.read_at(2, &mut buf).unwrap();
    assert_eq!(buf, [30, 40]);
    // Owner is still usable.
    assert_eq!(owned.size_bytes(), 4);
}

#[test]
fn arc_dyn_blockread_clones_share_state() {
    let inner = Arc::new(Tracker::new(vec![0xFF; 16], false));
    let a: Arc<dyn BlockRead> = inner.clone();
    let b: Arc<dyn BlockRead> = inner.clone();
    let mut buf = [0u8; 4];
    a.read_at(0, &mut buf).unwrap();
    b.read_at(4, &mut buf).unwrap();
    assert_eq!(*inner.reads.lock().unwrap(), 2);
}

/// `set_len` AND `can_grow` ARE FORWARDED, AND THE COUNTER IS WHY THIS
/// IS NOT VACUOUS.
///
/// Both are DEFAULTED methods on `BlockDevice`, and method resolution on
/// an `Arc<dyn BlockDevice>` finds `impl BlockDevice for Arc<T>` before
/// it derefs — so an impl that does not override them answers `false`
/// and `Err(ReadOnly)` over a device that can grow, with nothing to warn
/// anybody. Four image crates hold exactly this type at 33 sites between
/// them, which is what makes a missing forward here the difference
/// between shipping the growth API and shipping the shape of one.
///
/// So the assertion is that the call REACHED the device: `size_bytes`
/// moving could be a wrapper doing its own bookkeeping, but `grows`
/// belongs to the `Tracker` and only the `Tracker` can increment it.
#[test]
fn arc_blockdevice_forwards_set_len_and_can_grow() {
    let inner = Arc::new(Tracker::new(vec![0u8; 8], true));
    let dev: Arc<dyn BlockDevice> = inner.clone();

    assert!(
        dev.can_grow(),
        "Arc<dyn BlockDevice> did not forward can_grow"
    );
    dev.set_len(24)
        .expect("Arc<dyn BlockDevice> must forward set_len");

    assert_eq!(
        *inner.grows.lock().unwrap(),
        1,
        "the call reached the device"
    );
    assert_eq!(dev.size_bytes(), 24);
    let mut buf = [0xFFu8; 16];
    dev.read_at(8, &mut buf)
        .expect("the new region is readable");
    assert_eq!(buf, [0u8; 16]);
}

/// The same through `Box<T>`, which is a separate impl with its own
/// chance to forget.
#[test]
fn box_blockdevice_forwards_set_len_and_can_grow() {
    let dev: Box<dyn BlockDevice> = Box::new(Tracker::new(vec![0u8; 4], true));

    assert!(
        dev.can_grow(),
        "Box<dyn BlockDevice> did not forward can_grow"
    );
    dev.set_len(12)
        .expect("Box<dyn BlockDevice> must forward set_len");
    assert_eq!(dev.size_bytes(), 12);
}

/// THE CONTROL FOR BOTH. An impl that answered `true` and `Ok(())`
/// without asking the device would satisfy the two arms above; this one
/// wraps a device that refuses and asserts the refusal survives.
#[test]
fn the_forwarding_impls_do_not_invent_a_growth_capability() {
    let inner = Arc::new(Tracker::new(vec![0u8; 8], false));
    let dev: Arc<dyn BlockDevice> = inner.clone();

    assert!(!dev.can_grow());
    match dev.set_len(24) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected ReadOnly, got {other:?}"),
    }
    assert_eq!(*inner.grows.lock().unwrap(), 0);
    assert_eq!(dev.size_bytes(), 8, "and the device was not resized");
}
