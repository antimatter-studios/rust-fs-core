//! End-to-end tests for FileDevice + CallbackDevice + CachingDevice.

use fs_core::{BlockDevice, BlockRead, CachingDevice, CallbackDevice, Error, FileDevice};
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

fn tmp_image(bytes: &[u8]) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("fs_core_block_test_{}_{n}.img", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let mut f = File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    path
}

#[test]
fn file_device_ro_write_rejected() {
    let path = tmp_image(&[0u8; 4096]);
    let dev = FileDevice::open(&path).unwrap();
    assert!(!dev.is_writable());
    match dev.write_at(0, &[1u8; 16]) {
        Err(Error::ReadOnly) => {}
        other => panic!("expected ReadOnly, got {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn file_device_rw_round_trip() {
    let path = tmp_image(&[0u8; 4096]);
    let dev = FileDevice::open_rw(&path).unwrap();
    assert!(dev.is_writable());
    dev.write_at(100, &[0xAB, 0xCD, 0xEF]).unwrap();
    dev.flush().unwrap();
    let mut buf = [0u8; 3];
    dev.read_at(100, &mut buf).unwrap();
    assert_eq!(buf, [0xAB, 0xCD, 0xEF]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn callback_device_without_writer_rejects_writes() {
    let dev = CallbackDevice {
        size: 4096,
        read: Box::new(|_, buf| {
            buf.fill(0);
            Ok(())
        }),
        write: None,
        flush: None,
    };
    assert!(!dev.is_writable());
    assert!(dev.write_at(0, &[0u8; 4]).is_err());
}

// ---------------------------------------------------------------------------
// CachingDevice
// ---------------------------------------------------------------------------

struct CountingDev {
    size: u64,
    read_calls: Mutex<u64>,
    bytes: Mutex<Vec<u8>>,
}
impl CountingDev {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            size: bytes.len() as u64,
            read_calls: Mutex::new(0),
            bytes: Mutex::new(bytes),
        }
    }
}
impl BlockRead for CountingDev {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        *self.read_calls.lock().unwrap() += 1;
        let b = self.bytes.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        buf.copy_from_slice(&b[start..end]);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.size
    }
}
impl BlockDevice for CountingDev {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::Result<()> {
        let mut b = self.bytes.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        b[start..end].copy_from_slice(buf);
        Ok(())
    }
    fn is_writable(&self) -> bool {
        true
    }
}

#[test]
fn caching_device_caches_repeated_reads() {
    let bytes = (0u8..=255u8).cycle().take(64 * 1024).collect::<Vec<_>>();
    let inner: Arc<CountingDev> = Arc::new(CountingDev::new(bytes.clone()));
    let inner_trait: Arc<dyn BlockDevice> = inner.clone();
    let dev = CachingDevice::new(inner_trait, 4096, 4);

    let mut buf = vec![0u8; 4096];
    dev.read_at(0, &mut buf).unwrap();
    assert_eq!(&buf, &bytes[0..4096]);
    dev.read_at(0, &mut buf).unwrap();
    dev.read_at(0, &mut buf).unwrap();

    assert_eq!(*inner.read_calls.lock().unwrap(), 1);
    let (hits, misses) = dev.stats();
    assert_eq!((hits, misses), (2, 1));
}

/// A small read at an arbitrary offset is served from the block it
/// falls in, and the second one costs nothing.
///
/// This test asserted the opposite until the cache learned to slice:
/// a read that was not exactly one aligned block went straight to the
/// device, every time. That is almost every metadata read a filesystem
/// driver makes — an inode, a directory entry, a B+tree key — so the
/// cache was being bypassed by the traffic it existed for.
#[test]
fn a_small_unaligned_read_is_served_from_its_block() {
    let bytes = (0u8..=255u8).cycle().take(8192).collect::<Vec<_>>();
    let inner: Arc<CountingDev> = Arc::new(CountingDev::new(bytes.clone()));
    let inner_trait: Arc<dyn BlockDevice> = inner.clone();
    let dev = CachingDevice::new(inner_trait, 4096, 2);

    let mut buf = vec![0u8; 100];
    dev.read_at(123, &mut buf).unwrap();
    assert_eq!(&buf, &bytes[123..223], "the wrong bytes came back");
    dev.read_at(123, &mut buf).unwrap();
    assert_eq!(&buf, &bytes[123..223]);

    // One block fetched once, and the repeat came out of it.
    assert_eq!(*inner.read_calls.lock().unwrap(), 1);
    let (hits, misses) = dev.stats();
    assert_eq!((hits, misses), (1, 1));
}

/// A read that straddles a block boundary is stitched from both.
#[test]
fn a_read_across_a_boundary_is_stitched_from_both_blocks() {
    let bytes = (0u8..=255u8).cycle().take(16384).collect::<Vec<_>>();
    let inner: Arc<CountingDev> = Arc::new(CountingDev::new(bytes.clone()));
    let inner_trait: Arc<dyn BlockDevice> = inner.clone();
    let dev = CachingDevice::new(inner_trait, 4096, 8);

    let mut buf = vec![0u8; 200];
    dev.read_at(4000, &mut buf).unwrap();
    assert_eq!(&buf, &bytes[4000..4200]);
    assert_eq!(*inner.read_calls.lock().unwrap(), 2, "two blocks, once each");

    // Both halves are now cached, so neither is fetched again.
    dev.read_at(4000, &mut buf).unwrap();
    assert_eq!(&buf, &bytes[4000..4200]);
    assert_eq!(*inner.read_calls.lock().unwrap(), 2);
}

/// A read big enough to sweep the cache goes straight to the device.
///
/// Caching it would evict everything to hold bytes the caller has
/// already got, which costs the next reader and buys this one nothing.
#[test]
fn a_read_that_would_sweep_the_cache_bypasses_it() {
    let bytes = vec![0x5A; 32768];
    let inner: Arc<CountingDev> = Arc::new(CountingDev::new(bytes));
    let inner_trait: Arc<dyn BlockDevice> = inner.clone();
    let dev = CachingDevice::new(inner_trait, 4096, 2);

    // Two blocks against a two-block cache: more than half of it.
    let mut buf = vec![0u8; 8192];
    dev.read_at(0, &mut buf).unwrap();
    dev.read_at(0, &mut buf).unwrap();

    assert_eq!(*inner.read_calls.lock().unwrap(), 2, "each read went out");
    let (hits, misses) = dev.stats();
    assert_eq!((hits, misses), (0, 0), "nothing was cached either way");
}

#[test]
fn caching_device_invalidates_on_write() {
    let bytes = vec![0u8; 8192];
    let inner: Arc<CountingDev> = Arc::new(CountingDev::new(bytes));
    let inner_trait: Arc<dyn BlockDevice> = inner.clone();
    let dev = CachingDevice::new(inner_trait, 4096, 4);

    let mut buf = vec![0u8; 4096];
    dev.read_at(0, &mut buf).unwrap();
    dev.write_at(0, &[0xABu8; 4096]).unwrap();
    dev.read_at(0, &mut buf).unwrap();
    assert_eq!(buf[0], 0xAB);

    let (hits, misses) = dev.stats();
    assert_eq!(hits, 0);
    assert_eq!(misses, 2);
}
