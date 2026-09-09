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

/// A device with no bytes behind it and no lock in front of it.
///
/// `CountingDev` above holds its contents in a `Vec` behind a `Mutex`,
/// which is right for the eviction tests and wrong for the timing one:
/// a 32 MiB allocation and a mutex acquisition per read would be a
/// large part of what got measured. This one answers from nothing.
struct Zeros(u64);
impl BlockRead for Zeros {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> Result<()> {
        buf.fill(0xAB);
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        self.0
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

/// A BIGGER CACHE MUST NOT BE A SLOWER ONE.
///
/// The defect this pins: the LRU was a `VecDeque` scanned linearly and
/// promoted with `VecDeque::remove`, so every hit cost about
/// `capacity/2` comparisons and `capacity/2` tuple moves under the
/// mutex. The cost per read was linear in the capacity, which means
/// there was a capacity beyond which asking for more cache made the
/// driver slower, and no way for a caller to know where it was.
///
/// # Why a RATIO and not a wall-clock budget
///
/// An absolute threshold would encode this machine's speed and the
/// build profile, and would have to be loose enough for the slowest
/// runner -- at which point it stops failing on the defect. The two
/// capacities are measured in the SAME process and the SAME build, so
/// constant factors divide out and what is left is the shape of the
/// curve. Measured with the identical harness:
///
/// ```text
///                 us/read at 64   us/read at 4096   ratio
/// VecDeque, release      0.0387            1.5933     41x
/// VecDeque, debug        0.0661            3.6798     56x
/// index+list, release    0.0386            0.0459    1.19x
/// index+list, debug      0.0419            0.0613    1.46x
/// ```
///
/// The threshold sits at 8x: about five times above the worst passing
/// measurement and about five times below the best failing one, in
/// either profile. It is deliberately not tight -- this is a guard
/// against a return to O(n), not a benchmark.
///
/// The working set equals the capacity so the steady state is all
/// hits, and `CountingDevice` underneath asserts no device reads
/// happen during the timed section: without that, a change in miss
/// rate could pay for the whole difference and the test would be
/// measuring the wrong thing entirely.
#[test]
fn the_cost_of_a_cached_read_does_not_grow_with_the_capacity() {
    use fs_core::CountingDevice;
    use std::time::Instant;

    const BS: u64 = 4096;
    const BLOCKS: u64 = 8192;
    const READS: usize = 100_000;

    fn per_read_micros(capacity: usize) -> f64 {
        let counting = Arc::new(CountingDevice::new(Arc::new(Zeros(BS * BLOCKS))));
        let cache = CachingDevice::read_only(counting.clone(), BS, capacity);
        let mut buf = vec![0u8; 512];
        // Warm the whole working set, then measure only hits.
        for b in 0..capacity as u64 {
            cache.read_at(b * BS, &mut buf).unwrap();
        }
        let device_reads_after_warm = counting.reads();
        let (h0, m0) = cache.stats();

        let sweeps = READS / capacity.max(1);
        let reads = sweeps * capacity;
        let started = Instant::now();
        for _ in 0..sweeps {
            for b in 0..capacity as u64 {
                cache.read_at(b * BS, &mut buf).unwrap();
            }
        }
        let elapsed = started.elapsed().as_secs_f64() * 1e6;

        let (h1, m1) = cache.stats();
        assert_eq!(
            m1, m0,
            "capacity {capacity}: the timed section must be all hits, or this measures \
             device traffic rather than the cache's own bookkeeping"
        );
        assert_eq!(
            counting.reads(),
            device_reads_after_warm,
            "capacity {capacity}: no device read may happen during the timed section"
        );
        assert_eq!(
            h1 - h0,
            reads as u64,
            "capacity {capacity}: every read a hit"
        );
        elapsed / reads as f64
    }

    let small = per_read_micros(64);
    let large = per_read_micros(4096);
    let ratio = large / small.max(f64::MIN_POSITIVE);
    assert!(
        ratio < 8.0,
        "a cached read at capacity 4096 cost {large:.4} us against {small:.4} us at \
         capacity 64 -- {ratio:.1}x. The per-read cost is growing with the capacity, \
         which is the O(n) LRU this test exists to keep out: measured at 41x (release) \
         and 56x (debug) with a linearly-scanned VecDeque, and 1.2x-1.5x with an index \
         plus an intrusive recency list."
    );
}
