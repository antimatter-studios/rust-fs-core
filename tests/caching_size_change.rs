//! `CachingDevice::read_at` must not report success over a buffer it did
//! not fill.
//!
//! The stitching loop ends in an unconditional `Ok(())`. If a cached block
//! is shorter than the block size — which is what the last block of a
//! device looks like — and the device's reported size later says the read
//! is in range anyway, the loop runs out of bytes, `break`s, and the caller
//! is handed `Ok` and a buffer whose tail still holds whatever it held
//! before the call. That is the one outcome `read_at`'s own comment swears
//! off: "a short answer with no error, which is worse than the failure the
//! caller would otherwise have seen".

use fs_core::block::BlockRead;
use fs_core::error::Result;
use fs_core::CachingDevice;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

mod common;

/// A device holding 64 real bytes whose *reported* size is whatever was
/// last stored in the atomic.
///
/// THIS DEVICE BREAKS THE CONTRACT ON PURPOSE. `BlockRead::size_bytes`
/// does carry a stability contract — see the trait's own documentation,
/// which this comment used to contradict outright by claiming there was
/// none — and the point of this fixture is that `CachingDevice` catches a
/// device violating it rather than trusting it. `caching_device.rs` puts
/// it the right way round: "this is the device breaking its promise being
/// caught rather than believed". A device like this one in a real driver
/// is a defect; here it is the test subject.
struct ResizingDevice {
    data: Vec<u8>,
    reported: AtomicU64,
}

impl BlockRead for ResizingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        common::read_into(&self.data, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.reported.load(Ordering::SeqCst)
    }
}

#[test]
fn a_read_the_cache_could_not_fill_is_an_error_not_a_short_ok() {
    let dev = Arc::new(ResizingDevice {
        data: (0..64u8).collect(),
        reported: AtomicU64::new(8),
    });
    let cache = CachingDevice::read_only(dev.clone(), 16, 8);

    // Warm block 0 while the device says it holds 8 bytes. The block is
    // clamped to the device, so the cache now holds an 8-byte block 0.
    let mut warm = [0u8; 8];
    cache.read_at(0, &mut warm).expect("warming read");
    assert_eq!(warm, [0, 1, 2, 3, 4, 5, 6, 7]);

    // The device now says it is larger. Nothing invalidates the short block
    // that is still resident.
    dev.reported.store(64, Ordering::SeqCst);

    // 16 bytes at offset 0 is inside the device as it now reports itself,
    // so the pass-through guard does not fire and the read is served from
    // the cache — which holds only the first 8 of them.
    let mut buf = [0xEEu8; 16];
    let result = cache.read_at(0, &mut buf);

    assert!(
        result.is_err(),
        "read_at returned {result:?} over a buffer it filled only half of: {buf:?}"
    );
}
