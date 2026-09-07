//! A wild offset must be refused, not added to.
//!
//! `CachingDevice::read_at` computed `offset + buf.len()` — twice, and
//! unchecked — before reaching the one bounds check it has. In a debug
//! build that panics and the panic escapes the device, the driver, and any
//! host behind the C ABI without a guard of its own. In release it wraps,
//! and the read survives on the accident that the wrapped value usually
//! lands in the pass-through branch.
//!
//! A cache's offset is computed by a driver from on-disk fields — an
//! extent pointer, an inode block number, a directory offset — so a wild
//! offset is an ordinary thing to be handed off a corrupt or hostile
//! image, not a programming mistake in the caller. This is the same defect
//! `slice.rs` was fixed for, and its reasoning applies here verbatim.

use fs_core::block::BlockRead;
use fs_core::error::{Error, Result};
use fs_core::CachingDevice;
use std::sync::Arc;

/// Deliberately checked, so a panic in this test can only have come from
/// the cache and never from the device underneath it.
struct Bytes(Vec<u8>);

impl BlockRead for Bytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(buf.len());
        if end > self.0.len() || start > self.0.len() {
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

fn cache() -> Arc<CachingDevice> {
    let inner = Arc::new(Bytes((0..4096u32).map(|i| i as u8).collect()));
    CachingDevice::read_only(inner, 512, 4)
}

#[test]
fn a_read_at_the_very_top_of_the_address_space_is_refused() {
    let mut buf = [0u8; 8];
    let result = cache().read_at(u64::MAX, &mut buf);
    assert!(result.is_err(), "expected an error, got {result:?}");
}

#[test]
fn a_read_whose_end_wraps_past_the_top_is_refused() {
    let mut buf = [0u8; 64];
    let result = cache().read_at(u64::MAX - 4, &mut buf);
    assert!(result.is_err(), "expected an error, got {result:?}");
}

#[test]
fn an_ordinary_read_is_unaffected() {
    let mut buf = [0u8; 8];
    cache().read_at(16, &mut buf).expect("an ordinary read");
    assert_eq!(buf, [16, 17, 18, 19, 20, 21, 22, 23]);
}
