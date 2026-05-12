//! Forwarding impls of BlockRead / BlockDevice for Arc<T>, Box<T>, &T.

use fs_core::{BlockDevice, BlockRead, Error, Result};
use std::sync::{Arc, Mutex};

struct Tracker {
    bytes: Mutex<Vec<u8>>,
    reads: Mutex<u64>,
    writes: Mutex<u64>,
    flushes: Mutex<u64>,
    writable: bool,
}
impl Tracker {
    fn new(bytes: Vec<u8>, writable: bool) -> Self {
        Self {
            bytes: Mutex::new(bytes),
            reads: Mutex::new(0),
            writes: Mutex::new(0),
            flushes: Mutex::new(0),
            writable,
        }
    }
}
impl BlockRead for Tracker {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        *self.reads.lock().unwrap() += 1;
        let b = self.bytes.lock().unwrap();
        let s = offset as usize;
        buf.copy_from_slice(&b[s..s + buf.len()]);
        Ok(())
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
