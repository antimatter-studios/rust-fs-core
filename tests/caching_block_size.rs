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

mod common {
    use fs_core::block::BlockRead;
    use fs_core::error::{Error, Result};

    pub struct Bytes(pub Vec<u8>);

    impl BlockRead for Bytes {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            let end = start.saturating_add(buf.len());
            if end > self.0.len() {
                return Err(Error::ShortRead {
                    offset,
                    want: buf.len(),
                    got: self.0.len().saturating_sub(start),
                });
            }
            buf.copy_from_slice(&self.0[start..end]);
            Ok(())
        }

        fn size_bytes(&self) -> u64 {
            self.0.len() as u64
        }
    }
}

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
        let start = offset as usize;
        let end = start.saturating_add(buf.len());
        if end > d.len() {
            return Err(fs_core::error::Error::ShortRead {
                offset,
                want: buf.len(),
                got: d.len().saturating_sub(start),
            });
        }
        buf.copy_from_slice(&d[start..end]);
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.data.lock().unwrap().len() as u64
    }
}

impl BlockDevice for CountingRw {
    fn write_at(&self, offset: u64, buf: &[u8]) -> fs_core::error::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut d = self.data.lock().unwrap();
        let start = offset as usize;
        d[start..start + buf.len()].copy_from_slice(buf);
        Ok(())
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
