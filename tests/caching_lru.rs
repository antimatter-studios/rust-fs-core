//! LRU eviction behaviour and hit/miss accounting through eviction.

use fs_core::{BlockDevice, BlockRead, CachingDevice, Result};
use std::sync::{Arc, Mutex};

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
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        *self.read_calls.lock().unwrap() += 1;
        let b = self.bytes.lock().unwrap();
        let s = offset as usize;
        buf.copy_from_slice(&b[s..s + buf.len()]);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.size
    }
}
impl BlockDevice for CountingDev {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.bytes.lock().unwrap();
        let s = offset as usize;
        b[s..s + buf.len()].copy_from_slice(buf);
        Ok(())
    }
    fn is_writable(&self) -> bool {
        true
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[test]
fn capacity_one_evicts_on_every_new_block() {
    let inner = Arc::new(CountingDev::new(pattern(16 * 1024)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let bs = 4096u64;
    let cache = CachingDevice::new(inner_d, bs, 1);

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // miss
    cache.read_at(0, &mut buf).unwrap(); // hit
    cache.read_at(bs, &mut buf).unwrap(); // miss; evicts block 0
    cache.read_at(0, &mut buf).unwrap(); // miss; block 0 was evicted

    let (hits, misses) = cache.stats();
    assert_eq!((hits, misses), (1, 3));
    assert_eq!(*inner.read_calls.lock().unwrap(), 3);
}

#[test]
fn lru_evicts_least_recently_used_not_first_inserted() {
    let inner = Arc::new(CountingDev::new(pattern(64 * 1024)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let bs = 4096u64;
    let cache = CachingDevice::new(inner_d, bs, 2);

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // miss, cache=[0]
    cache.read_at(bs, &mut buf).unwrap(); // miss, cache=[bs, 0]
    cache.read_at(0, &mut buf).unwrap(); // hit, cache=[0, bs]  (0 is now MRU)
    cache.read_at(2 * bs, &mut buf).unwrap(); // miss, evicts bs (LRU), cache=[2bs, 0]
    cache.read_at(0, &mut buf).unwrap(); // hit, still cached
    cache.read_at(bs, &mut buf).unwrap(); // miss, was evicted

    let (hits, misses) = cache.stats();
    assert_eq!((hits, misses), (2, 4));
}

#[test]
fn invalidate_all_clears_cache() {
    let inner = Arc::new(CountingDev::new(pattern(8192)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let bs = 4096u64;
    let cache = CachingDevice::new(inner_d, bs, 4);

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap();
    cache.read_at(0, &mut buf).unwrap();
    let (h1, m1) = cache.stats();
    assert_eq!((h1, m1), (1, 1));

    cache.invalidate_all();
    cache.read_at(0, &mut buf).unwrap(); // miss again

    let (h2, m2) = cache.stats();
    assert_eq!((h2, m2), (1, 2));
}

#[test]
fn write_invalidates_overlapping_cache_blocks() {
    let inner = Arc::new(CountingDev::new(pattern(16 * 1024)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let bs = 4096u64;
    let cache = CachingDevice::new(inner_d, bs, 4);

    let mut buf = vec![0u8; bs as usize];
    cache.read_at(0, &mut buf).unwrap(); // populate block 0
    cache.read_at(bs, &mut buf).unwrap(); // populate block 1
    cache.read_at(2 * bs, &mut buf).unwrap(); // populate block 2

    // Write that spans block 0 only. Block 1 + 2 should survive.
    cache.write_at(100, &[0xFFu8; 32]).unwrap();
    cache.read_at(bs, &mut buf).unwrap(); // hit
    cache.read_at(2 * bs, &mut buf).unwrap(); // hit
    cache.read_at(0, &mut buf).unwrap(); // miss (invalidated)

    let (hits, misses) = cache.stats();
    // 3 initial misses + 2 hits + 1 fresh miss = (2, 4).
    assert_eq!((hits, misses), (2, 4));
}

/// Non-aligned and partial reads go through the cache like any other.
///
/// This test used to assert the reverse — that neither counted as a hit
/// or a miss, because both bypassed the cache entirely. That was the
/// defect: a driver's metadata reads are almost all unaligned or
/// smaller than a block, so the cache never saw the traffic it exists
/// for.
#[test]
fn non_aligned_and_partial_reads_go_through_the_cache() {
    let inner = Arc::new(CountingDev::new(pattern(8192)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let bs = 4096u64;
    let cache = CachingDevice::new(inner_d, bs, 4);

    // Non-aligned offset, a block's worth: straddles blocks 0 and 1, so
    // both are fetched and held.
    let mut buf = vec![0u8; bs as usize];
    cache.read_at(123, &mut buf).unwrap();
    assert_eq!(cache.stats(), (0, 2), "two blocks, neither held yet");

    // Partial size, inside block 0 — which is now cached.
    let mut small = vec![0u8; 64];
    cache.read_at(0, &mut small).unwrap();
    assert_eq!(&small[..], &pattern(8192)[0..64]);

    let (hits, misses) = cache.stats();
    assert_eq!((hits, misses), (1, 2));
    // The second read never reached the device.
    assert_eq!(*inner.read_calls.lock().unwrap(), 2);
}

#[test]
fn forwards_size_bytes_and_is_writable_from_inner() {
    let inner = Arc::new(CountingDev::new(pattern(4096)));
    let inner_d: Arc<dyn BlockDevice> = inner.clone();
    let cache = CachingDevice::new(inner_d, 4096, 2);
    assert_eq!(cache.size_bytes(), 4096);
    assert!(cache.is_writable());
}
