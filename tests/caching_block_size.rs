//! A `CachingDevice`'s block size comes off a disk, so it has to be checked.
//!
//! Both constructors take any `u64`. Zero divides by zero on the first read
//! — which panics unconditionally in Rust, so unlike the wrapping
//! arithmetic elsewhere in `read_at` this one takes down a release build
//! too. At the other end of the range an absurd value turns the eager
//! block allocation in `block()` into a request for as much memory as the
//! device is large.
//!
//! Neither is hypothetical. The clamp in `block()` exists because
//! `am-fs-squashfs` declares its block size from the archive's superblock,
//! and a field that comes off the disk is a field a truncated, fuzzed or
//! hostile image can set to anything.

use fs_core::block::{BlockDevice, BlockRead};
use fs_core::CachingDevice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod common;

fn backing() -> Arc<common::Bytes> {
    Arc::new(common::Bytes((0..4096u32).map(|i| i as u8).collect()))
}

#[test]
fn a_zero_block_size_is_refused_rather_than_divided_by() {
    let cache = CachingDevice::read_only(backing(), 0, 4);
    let mut buf = [0u8; 8];
    let result = cache.read_at(0, &mut buf);
    assert!(
        result.is_err(),
        "a zero block size must be refused, not divided by; got {result:?}"
    );
}

#[test]
fn an_absurd_block_size_is_refused_rather_than_allocated_for() {
    // A terabyte block size over a 4 KiB device. The clamp in `block()`
    // stops this particular case allocating a terabyte only because the
    // device is small; over a large image it would not.
    let cache = CachingDevice::read_only(backing(), 1 << 40, 4);
    let mut buf = [0u8; 8];
    let result = cache.read_at(0, &mut buf);
    assert!(
        result.is_err(),
        "a block size no device could justify must be refused; got {result:?}"
    );
}

#[test]
fn an_ordinary_block_size_still_works() {
    // The guard must not catch the case the clamp exists for: a declared
    // block size much larger than the whole image.
    let small = Arc::new(common::Bytes(vec![0xAB; 4096]));
    let cache = CachingDevice::read_only(small, 128 * 1024, 4);
    let mut buf = [0u8; 8];
    cache
        .read_at(0, &mut buf)
        .expect("128 KiB blocks over a 4 KiB image");
    assert_eq!(buf, [0xAB; 8]);
}

/// A writable device that counts what actually reached it.
struct CountingRw {
    data: std::sync::Mutex<Vec<u8>>,
    writes: AtomicUsize,
}

impl BlockRead for CountingRw {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::error::Result<()> {
        let d = self.data.lock().unwrap();
        common::read_into(&d, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.data.lock().unwrap().len() as u64
    }
}

impl BlockDevice for CountingRw {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::error::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut d = self.data.lock().unwrap();
        common::write_from(&mut d, offset, buf)
    }

    fn is_writable(&self) -> bool {
        true
    }
}

/// A write is refused too, and refused BEFORE the device is reached.
///
/// `write_at` sweeps the cache twice, once either side of the device
/// write, and that pair is what stops a concurrent read leaving pre-write
/// bytes behind. The block-size refusal has to sit above both of them.
///
/// Above the first because `invalidate_range` computes
/// `block_end = off + block_size`: with a zero block size `block_end`
/// equals `off`, the retain predicate becomes `*off >= end || *off <=
/// start`, and the sweep KEEPS entries the write has made stale. Above
/// the second because the guarantee rests on "if the device was written,
/// both sweeps ran", and an early return below the device write would
/// break it.
///
/// The half of that which is observable from outside is this: the refusal
/// must come before anything reaches the device. The rest is unobservable
/// by construction, because a cache that refuses every read can never
/// hold an entry for a sweep to mishandle.
#[test]
fn a_zero_block_size_refuses_a_write_before_it_reaches_the_device() {
    let dev = Arc::new(CountingRw {
        data: std::sync::Mutex::new(vec![0xAA; 64]),
        writes: AtomicUsize::new(0),
    });
    let cache = CachingDevice::new(dev.clone(), 0, 4);

    let result = cache.write_at(0, &[0xBB; 8]);

    assert!(
        result.is_err(),
        "a zero block size must refuse a write; got {result:?}"
    );
    assert_eq!(
        dev.writes.load(Ordering::SeqCst),
        0,
        "the refusal must happen before the device is reached"
    );
    assert_eq!(
        *dev.data.lock().unwrap(),
        vec![0xAA; 64],
        "and nothing may have been written"
    );
}

/// The control for the one above: an ordinary block size still writes.
#[test]
fn an_ordinary_block_size_still_writes() {
    let dev = Arc::new(CountingRw {
        data: std::sync::Mutex::new(vec![0xAA; 64]),
        writes: AtomicUsize::new(0),
    });
    let cache = CachingDevice::new(dev.clone(), 16, 4);

    cache.write_at(0, &[0xBB; 8]).expect("an ordinary write");

    assert_eq!(dev.writes.load(Ordering::SeqCst), 1);
    assert_eq!(&dev.data.lock().unwrap()[..8], &[0xBB; 8]);
}

/// A read-only device that counts the reads that reached it.
struct CountingRo {
    data: Vec<u8>,
    reads: AtomicUsize,
}

impl BlockRead for CountingRo {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::error::Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        common::read_into(&self.data, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.data.len() as u64
    }
}

fn counting_ro() -> Arc<CountingRo> {
    Arc::new(CountingRo {
        data: vec![0xAA; 64],
        reads: AtomicUsize::new(0),
    })
}

/// A READ-ONLY CACHE REFUSES A WRITE WITH `ReadOnly`, WHATEVER ITS BLOCK
/// SIZE SAYS.
///
/// The block size is checked before anything else in `write_at`, so a
/// cache with no writable half whose block size came off a damaged disk
/// used to answer `Error::Custom` instead — against this type's own
/// documented promise that such a write is `Error::ReadOnly`.
///
/// The variant is not cosmetic. `stream.rs` maps `ReadOnly` to
/// `io::ErrorKind::PermissionDenied` and `Custom` to `io::Error::other`,
/// so a caller branching on `PermissionDenied` to report "this volume is
/// read-only" reported an uncategorised failure. A read-only mount of a
/// damaged image is precisely where a bad block size shows up, so the two
/// conditions arrive together rather than independently.
///
/// Both ends of the range, because the block-size guard has two arms and
/// either would have masked the contract.
#[test]
fn a_read_only_cache_refuses_a_write_with_read_only_whatever_the_block_size() {
    for block_size in [0u64, fs_core::caching_device::MAX_BLOCK_SIZE + 1] {
        let cache = CachingDevice::read_only(counting_ro(), block_size, 4);
        let result = cache.write_at(0, &[0xBB; 8]);
        assert!(
            matches!(result, Err(fs_core::error::Error::ReadOnly)),
            "a read-only cache with block size {block_size} must refuse a write \
             with ReadOnly, or an io consumer sees Other instead of \
             PermissionDenied; got {result:?}"
        );
    }
}

/// THE OVER-CORRECTION CONTROL. A cache that CAN be written still reports
/// the block size.
///
/// Answering `ReadOnly` whenever the block size is unusable would satisfy
/// the test above and be wrong: it would tell a caller holding a writable
/// device that its volume is read-only, and hide the one fact that would
/// let anyone fix the image. The hoist has to move the read-only refusal
/// up, not turn the block-size refusal into a read-only one.
#[test]
fn a_writable_cache_with_a_bad_block_size_still_reports_the_block_size() {
    for block_size in [0u64, fs_core::caching_device::MAX_BLOCK_SIZE + 1] {
        let dev = Arc::new(CountingRw {
            data: std::sync::Mutex::new(vec![0xAA; 64]),
            writes: AtomicUsize::new(0),
        });
        let cache = CachingDevice::new(dev.clone(), block_size, 4);
        let result = cache.write_at(0, &[0xBB; 8]);
        assert!(
            matches!(result, Err(fs_core::error::Error::Custom(_))),
            "a WRITABLE cache with block size {block_size} must report the block \
             size, not claim the device is read-only; got {result:?}"
        );
        assert_eq!(
            dev.writes.load(Ordering::SeqCst),
            0,
            "and still nothing may reach the device"
        );
    }
}

/// A REFUSED WRITE LEAVES THE CACHE ALONE.
///
/// The read-only refusal used to sit below the first invalidation sweep,
/// so every write a read-only cache refused first emptied the region it
/// was refused for. The entries could not have been stale — the write
/// never reached the device and never could — so the re-read bought
/// nothing.
///
/// Counted in device reads rather than asserted about internals: prime an
/// entry, prove it is being served from the cache, refuse a write over it,
/// and require that it is still served from the cache.
#[test]
fn a_write_refused_on_a_read_only_cache_does_not_empty_the_cache() {
    let dev = counting_ro();
    let cache = CachingDevice::read_only(dev.clone(), 16, 4);
    let mut buf = [0u8; 8];

    cache.read_at(0, &mut buf).expect("prime the entry");
    let primed = dev.reads.load(Ordering::SeqCst);
    assert!(primed > 0, "priming must have reached the device");

    // PROVE THE ENTRY IS CACHED BEFORE RELYING ON IT. Without this the
    // assertion below would hold just as well for a cache that never
    // stored anything, and would be measuring nothing.
    cache.read_at(0, &mut buf).expect("served from the cache");
    assert_eq!(
        dev.reads.load(Ordering::SeqCst),
        primed,
        "the second read must be a hit, or this test cannot tell a surviving \
         entry from an absent one"
    );

    let refused = cache.write_at(0, &[0xBB; 8]);
    assert!(
        matches!(refused, Err(fs_core::error::Error::ReadOnly)),
        "the write must be refused; got {refused:?}"
    );

    cache.read_at(0, &mut buf).expect("still cached");
    assert_eq!(
        dev.reads.load(Ordering::SeqCst),
        primed,
        "a refused write swept the cache: the entry had to be fetched again, \
         though the write it was swept for never reached the device"
    );
    assert_eq!(buf, [0xAA; 8], "and the bytes are the device's own");
}
