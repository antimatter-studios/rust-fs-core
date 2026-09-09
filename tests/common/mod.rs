//! The bounds arithmetic every in-memory double in `tests/` shares.
//!
//! # Why this exists next to `src/test_device.rs` rather than inside it
//!
//! `src/test_device.rs` is `#[cfg(test)]`, and an integration test is a
//! separate crate that cannot see a `#[cfg(test)]` item. So the eleven
//! suites here each hand-rolled the arithmetic the shared device was
//! consolidated to own, and each of them re-introduced the divergence
//! that consolidation had removed:
//!
//! ```text
//! let s = offset as usize;
//! buf.copy_from_slice(&b[s..s + buf.len()]);
//! ```
//!
//! No bounds check at all, so a past-end read panicked inside the
//! double — which turns a caller's arithmetic bug into a crash in the
//! harness rather than an error the caller can be asserted against, and
//! means a test that wants to prove a wrapper REFUSES a wild offset
//! cannot be written with these devices at all. `tests/caching_wild_
//! offset.rs` had to declare a twelfth, "deliberately checked" double
//! for exactly that reason.
//!
//! The alternative was to publish the real one behind a
//! `#[cfg(feature = "test-support")]` module. That adds a feature and a
//! self dev-dependency to the manifest of a crate twelve siblings pin,
//! to ship test support nothing outside this repository consumes. One
//! file the suites share is the smaller change; the cost is that the
//! rule is stated twice, so both copies carry a test that pins it —
//! `tests/shared_double_bounds.rs` here, `src/test_device.rs::tests`
//! there.

#![allow(dead_code)]

use fs_core::block::BlockRead;
use fs_core::error::{Error, Result};

/// Where `offset` lands in a buffer of `len` bytes — or the
/// [`Error::ShortRead`] that says the range does not fit, which always
/// reports `got: 0` because nothing is copied on that path.
///
/// The arithmetic is `u64` throughout, and the narrowing to `usize`
/// happens only *after* the bounds check has proved the range fits
/// inside `len` — rather than before it, where `offset as usize` would
/// turn a wild offset into a plausible one instead of refusing it, and
/// where `start + want` panicked with "attempt to add with overflow"
/// in a debug build, which is every `cargo test` run.
pub fn range_within(len: usize, offset: u64, want: usize) -> Result<(usize, usize)> {
    let len64 = len as u64;
    let short = || Error::ShortRead {
        offset,
        want,
        // ZERO, ALWAYS — AND NOT "WHAT WAS AVAILABLE".
        //
        // `got` counts the bytes placed in the CALLER'S BUFFER. This
        // refuses before `copy_from_slice` runs, on every path that
        // builds this error, so the buffer is provably untouched and
        // the count is provably nothing.
        //
        // It reported `len - offset`, which is a different number
        // whenever the read starts inside the buffer and only overruns
        // at the far end: 16 bytes from offset 0 of an 8-byte double
        // claimed `got: 8` for a read that copied none. That is
        // `FileDevice`'s answer, not this one's. A file really does
        // hand back the readable prefix and report its length; the
        // slice adapters refuse outright and report 0; `error.rs`
        // spells out that `got: 0` means nothing was transferred and
        // says nothing about what was there. This double refuses, so
        // it is the second kind.
        got: 0,
    };
    let end = offset.checked_add(want as u64).ok_or_else(short)?;
    if end > len64 {
        return Err(short());
    }
    // `offset <= end <= len`, and `len` is a `usize`, so both of these
    // narrowings are exact.
    Ok((offset as usize, end as usize))
}

/// The read every double here shares: a past-end read is
/// [`Error::ShortRead`], never a panic, and a refused read leaves the
/// caller's buffer untouched.
pub fn read_into(bytes: &[u8], offset: u64, buf: &mut [u8]) -> Result<()> {
    let (start, end) = range_within(bytes.len(), offset, buf.len())?;
    buf.copy_from_slice(&bytes[start..end]);
    Ok(())
}

/// The write, which carried the identical pair of lines. A device is
/// not a `Vec`: a write past the end grows nothing and is refused.
pub fn write_from(bytes: &mut [u8], offset: u64, buf: &[u8]) -> Result<()> {
    let (start, end) = range_within(bytes.len(), offset, buf.len())?;
    bytes[start..end].copy_from_slice(buf);
    Ok(())
}

/// A read-only byte buffer — the plainest double there is, and the one
/// several suites need only in order to have *something* under the
/// device being tested.
pub struct Bytes(pub Vec<u8>);

impl BlockRead for Bytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        read_into(&self.0, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.0.len() as u64
    }
}
