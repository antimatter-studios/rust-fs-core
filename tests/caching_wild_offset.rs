//! A wild offset must be refused, not added to.
//!
//! `CachingDevice::read_at` computed `offset + buf.len()` — twice, and
//! unchecked — before reaching the one bounds check it has. In a debug
//! build that panics and the panic escapes the device, the driver, and
//! any host behind the C ABI without a guard of its own. In release it
//! wraps, and the read survives on the accident that the wrapped value
//! usually lands in the pass-through branch.
//!
//! # THE ASSERTION IS THAT THE DEVICE WAS NEVER CONSULTED
//!
//! These tests used to assert only `is_err()`, and that could not tell
//! a refusal from a forward. The device errors on these inputs too, and
//! not merely with some error — with the SAME one. Measured:
//!
//! ```text
//! offset=u64::MAX want=8:  device -> ShortRead{offset:u64::MAX, want:8, got:0}
//!                          cache  -> ShortRead{offset:u64::MAX, want:8, got:0}
//! ```
//!
//! `usize::try_from(u64::MAX)` succeeds on a 64-bit target, so `start`
//! becomes `usize::MAX`, `got` saturates to 0, and the two are byte for
//! byte the same value. Strengthening the assertion to compare the whole
//! variant therefore does not help either: a cache that forwarded the
//! wild offset would still pass.
//!
//! What separates the two is whether the device was asked at all. A
//! refusal is a decision the cache makes on its own; a forward is not.
//! So the device counts its reads, and the refusal tests assert that
//! count is zero — with `an_ordinary_read_consults_the_device` as the
//! control that stops a counter which never increments passing them
//! vacuously.

use fs_core::block::BlockRead;
use fs_core::error::{Error, Result};
use fs_core::CachingDevice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Deliberately checked, so a panic in this test can only have come from
/// the cache and never from the device underneath it — and counting, so
/// a test can tell whether it was reached.
struct Bytes {
    data: Vec<u8>,
    reads: AtomicUsize,
}

impl Bytes {
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl BlockRead for Bytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(buf.len());
        if end > self.data.len() || start > self.data.len() {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: self.data.len().saturating_sub(start),
            });
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.data.len() as u64
    }
}

fn cache() -> (Arc<CachingDevice>, Arc<Bytes>) {
    let inner = Arc::new(Bytes {
        data: (0..4096u32).map(|i| i as u8).collect(),
        reads: AtomicUsize::new(0),
    });
    (CachingDevice::read_only(inner.clone(), 512, 4), inner)
}

/// `read_at(u64::MAX, &mut [0u8; 8])`.
#[test]
fn a_read_at_the_very_top_of_the_address_space_is_refused() {
    let (cache, dev) = cache();
    let mut buf = [0xEEu8; 8];

    let result = cache.read_at(u64::MAX, &mut buf);

    match result {
        Err(Error::ShortRead { offset, want, got }) => {
            assert_eq!((offset, want, got), (u64::MAX, 8, 0));
        }
        other => panic!("expected ShortRead, got {other:?}"),
    }
    assert_eq!(
        dev.reads(),
        0,
        "the cache forwarded the wild offset to the device instead of refusing it"
    );
    assert_eq!(buf, [0xEEu8; 8], "a refused read leaves the buffer alone");
}

/// `read_at(u64::MAX - 4, &mut [0u8; 64])` — the sum wraps rather than
/// merely being large.
#[test]
fn a_read_whose_end_wraps_past_the_top_is_refused() {
    let (cache, dev) = cache();
    let mut buf = [0xEEu8; 64];

    let result = cache.read_at(u64::MAX - 4, &mut buf);

    match result {
        Err(Error::ShortRead { offset, want, got }) => {
            assert_eq!((offset, want, got), (u64::MAX - 4, 64, 0));
        }
        other => panic!("expected ShortRead, got {other:?}"),
    }
    assert_eq!(
        dev.reads(),
        0,
        "the cache forwarded the wrapping offset to the device instead of refusing it"
    );
    assert_eq!(buf, [0xEEu8; 64], "a refused read leaves the buffer alone");
}

/// The control for the two above. Without it a counter that never
/// incremented — a device the cache never calls at all, or a broken
/// counter — would satisfy both of them.
#[test]
fn an_ordinary_read_consults_the_device() {
    let (cache, dev) = cache();
    let mut buf = [0u8; 8];

    cache.read_at(16, &mut buf).expect("an ordinary read");

    assert_eq!(buf, [16, 17, 18, 19, 20, 21, 22, 23]);
    assert!(
        dev.reads() >= 1,
        "an ordinary read must reach the device, or the zero above proves nothing"
    );
}
