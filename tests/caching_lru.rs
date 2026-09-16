//! LRU eviction behaviour and hit/miss accounting through eviction.

use fs_core::{BlockDevice, BlockRead, CachingDevice, Result};
use std::sync::{Arc, Mutex};

mod common;

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
        common::read_into(&b, offset, buf)
    }
    fn size_bytes(&self) -> u64 {
        self.size
    }
}
impl BlockDevice for CountingDev {
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.bytes.lock().unwrap();
        common::write_from(&mut b, offset, buf)
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
/// runner -- at which point it stops failing on the defect. Both arms
/// are measured in the SAME process and the SAME build, so constant
/// factors divide out and what is left is the shape of the curve.
///
/// # Why the two arms hold THE SAME NUMBER OF BLOCKS (#99)
///
/// This used to time one cache of 64 blocks against one cache of 4096.
/// A ratio only cancels what both arms share, and those two did not
/// share their working set: 256 KiB, resident in L2 throughout, against
/// 16 MiB, which any other process on the machine evicts. Under load the
/// large arm slowed 6.9x and the small one 1.4x, and correct code
/// measured 8.4x -- then 22x-26x at load 40 -- against an 8x threshold
/// whose weakest true positive was 18.2x. No threshold separated them.
///
/// So now both arms hold 4096 blocks and every sampled read touches a
/// different one. The large arm is one cache of capacity 4096; the
/// small arm is 64 caches of capacity 64, read round-robin. What the
/// memory hierarchy does to one it does to the other, and the only thing
/// left to differ is how many entries one cache's bookkeeping has to
/// deal with.
///
/// # Why the MINIMUM of many short samples
///
/// A pre-emption only ever adds time, so the fastest of many samples is
/// the estimate least touched by whatever else the machine is doing. A
/// sample is 4096 reads -- a fraction of a millisecond at O(1), shorter
/// than a scheduler slice -- and the arms are interleaved, so a burst of
/// load lands on both rather than on whichever ran second. A single
/// long run per arm, which this used to be, is inflated by every slice
/// the thread lost during it.
///
/// # The threshold
///
/// Measured on a 4-core Cortex-A76, debug build, nine runs per row: at
/// ambient load 11, and with six and then eight extra processes copying
/// 64 MiB buffers in a loop (load 14 and 21):
///
/// ```text
///                                                   ratio, lowest .. highest
/// index + recency list (this crate)                          0.7 .. 1.0
/// O(n) contiguous scan of the slab to find the entry        16.7 .. 32.5
/// O(1) lookup, O(n) list walk to promote it                 38.1 .. 74.6
/// ```
///
/// The contiguous scan is the cheapest O(n) there is -- sequential
/// memory, no pointer chasing -- so it is the weakest true positive.
/// The threshold sits at 3x: three times the worst passing measurement
/// and more than five times below the weakest failing one. Under the
/// same eight-process load the previous two-capacity version of this
/// test failed correct code in 6 of 10 runs (11.1x-35.0x); this one
/// passed 10 of 10. It is a guard against a return to O(n), not a
/// benchmark.
///
/// Every sampled read must be a hit, and `CountingDevice` underneath
/// asserts no device read happens while sampling: without that, a
/// change in miss rate could pay for the whole difference and the test
/// would be measuring the wrong thing entirely.
#[test]
fn the_cost_of_a_cached_read_does_not_grow_with_the_capacity() {
    use fs_core::CountingDevice;
    use std::time::Instant;

    const BS: u64 = 512;
    const READ_LEN: usize = 64;
    /// Blocks held by each arm, and reads per sample.
    const ENTRIES: usize = 4096;
    const SMALL: usize = 64;
    const SAMPLES: usize = 50;

    /// `ENTRIES` blocks held across `ENTRIES / capacity` caches of
    /// `capacity` each, warmed.
    struct Arm {
        capacity: usize,
        caches: Vec<(Arc<CountingDevice>, Arc<CachingDevice>)>,
        buf: Vec<u8>,
    }

    impl Arm {
        fn new(capacity: usize) -> Self {
            let caches: Vec<_> = (0..ENTRIES / capacity)
                .map(|_| {
                    let counting = Arc::new(CountingDevice::new(Arc::new(Zeros(BS * 8192))));
                    let cache = CachingDevice::read_only(counting.clone(), BS, capacity);
                    (counting, cache)
                })
                .collect();
            let mut arm = Arm {
                capacity,
                caches,
                buf: vec![0u8; READ_LEN],
            };
            arm.sweep();
            arm
        }

        /// One read of every held block, each on a different block from
        /// the read before it. Round-robin across the caches, so the
        /// small arm moves through memory the way the large one does.
        fn sweep(&mut self) {
            let n = self.caches.len();
            for i in 0..ENTRIES {
                let (_, cache) = &self.caches[i % n];
                cache.read_at((i / n) as u64 * BS, &mut self.buf).unwrap();
            }
        }

        fn device_reads(&self) -> u64 {
            self.caches.iter().map(|(c, _)| c.reads()).sum()
        }

        fn stats(&self) -> (u64, u64) {
            self.caches.iter().fold((0, 0), |(h, m), (_, cache)| {
                let (ch, cm) = cache.stats();
                (h + ch, m + cm)
            })
        }

        /// Microseconds per read over one sweep.
        fn sample(&mut self) -> f64 {
            let started = Instant::now();
            self.sweep();
            started.elapsed().as_secs_f64() * 1e6 / ENTRIES as f64
        }
    }

    let mut small = Arm::new(SMALL);
    let mut large = Arm::new(ENTRIES);
    let before = [
        (small.device_reads(), small.stats()),
        (large.device_reads(), large.stats()),
    ];

    let (mut small_best, mut large_best) = (f64::MAX, f64::MAX);
    for _ in 0..SAMPLES {
        small_best = small_best.min(small.sample());
        large_best = large_best.min(large.sample());
    }

    for (arm, (reads, (h0, m0))) in [&small, &large].into_iter().zip(before) {
        let capacity = arm.capacity;
        let (h1, m1) = arm.stats();
        assert_eq!(
            m1, m0,
            "capacity {capacity}: sampling must be all hits, or this measures \
             device traffic rather than the cache's own bookkeeping"
        );
        assert_eq!(
            arm.device_reads(),
            reads,
            "capacity {capacity}: no device read may happen while sampling"
        );
        assert_eq!(
            h1 - h0,
            (SAMPLES * ENTRIES) as u64,
            "capacity {capacity}: every read a hit"
        );
    }

    let ratio = large_best / small_best.max(f64::MIN_POSITIVE);
    assert!(
        ratio < 3.0,
        "a cached read at capacity {ENTRIES} cost {large_best:.4} us against \
         {small_best:.4} us at capacity {SMALL}, both holding {ENTRIES} blocks -- \
         {ratio:.1}x. The per-read cost is growing with the capacity, which is the \
         O(n) LRU this test exists to keep out: measured at 16.7x-32.5x with a \
         contiguous linear scan, and 0.7x-1.0x with an index plus an intrusive \
         recency list."
    );
}
