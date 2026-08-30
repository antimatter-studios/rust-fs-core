//! In-memory devices for this crate's own tests.
//!
//! # What is shared, and what deliberately is not
//!
//! Four modules each declared a byte-buffer device — `stream::Bytes`,
//! `slice::Bytes`, `slice::RwBytes`, `readonly::WritableBytes` — and
//! three of the four were identical.
//!
//! The fourth was not, and the difference mattered: `WritableBytes`
//! read **without a bounds check**, so a read past the end panicked
//! where the other three returned [`Error::ShortRead`]. That is the
//! kind of divergence a consolidation has to find rather than flatten,
//! and it is why the eleven doubles were worth checking one at a time
//! instead of merging on sight.
//!
//! `ShortRead` is the right answer for all of them. A device that
//! panics on a past-end read turns a caller's arithmetic bug into a
//! crash in the test harness rather than an error the caller can be
//! asserted against.
//!
//! **Three doubles stay where they are**, because each exists to
//! misbehave in one specific way and a shared device is the opposite of
//! that: `stream::AlwaysFails` (every read errors), `ffi::Panicking`
//! (every method panics) and `tests/cache.rs::CountingDev` (counts
//! reads). Those are not duplicates; they are the point of their tests.

use crate::block::{BlockDevice, BlockRead};
use crate::error::{Error, Result};
use std::sync::Mutex;

/// A read-only byte buffer.
pub(crate) struct Bytes(pub Mutex<Vec<u8>>);

impl Bytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Bytes(Mutex::new(bytes))
    }
}

impl BlockRead for Bytes {
    /// A read past the end is [`Error::ShortRead`], carrying how much
    /// was actually available — not a zero-fill, and not a panic.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.0.lock().unwrap();
        read_into(&b, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
}

/// A writable byte buffer, with the same read behaviour.
pub(crate) struct RwBytes(pub Mutex<Vec<u8>>);

impl RwBytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        RwBytes(Mutex::new(bytes))
    }
}

impl BlockRead for RwBytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let b = self.0.lock().unwrap();
        read_into(&b, offset, buf)
    }

    fn size_bytes(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
}

impl BlockDevice for RwBytes {
    /// A write past the end grows nothing and reports what it could
    /// have taken — a device is not a `Vec`.
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut b = self.0.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        if end > b.len() {
            return Err(Error::ShortRead {
                offset,
                want: buf.len(),
                got: b.len().saturating_sub(start),
            });
        }
        b[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn is_writable(&self) -> bool {
        true
    }
}

/// The one read implementation both share.
fn read_into(b: &[u8], offset: u64, buf: &mut [u8]) -> Result<()> {
    let start = offset as usize;
    let end = start + buf.len();
    if end > b.len() {
        return Err(Error::ShortRead {
            offset,
            want: buf.len(),
            got: b.len().saturating_sub(start),
        });
    }
    buf.copy_from_slice(&b[start..end]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The behaviour the four copies were supposed to share, and one
    /// did not.
    #[test]
    fn a_read_past_the_end_is_a_short_read_not_a_panic() {
        let dev = Bytes::new(vec![0xAB; 8]);
        let mut buf = [0u8; 16];
        match dev.read_at(0, &mut buf).expect_err("past the end") {
            Error::ShortRead { offset, want, got } => assert_eq!((offset, want, got), (0, 16, 8)),
            other => panic!("expected ShortRead, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 16], "a refused read leaves the buffer alone");
    }

    #[test]
    fn the_writable_one_reads_the_same_way() {
        let dev = RwBytes::new(vec![0xAB; 8]);
        let mut buf = [0u8; 16];
        assert!(dev.read_at(0, &mut buf).is_err());
        assert!(dev.is_writable());
    }

    #[test]
    fn a_write_past_the_end_is_refused_rather_than_growing_the_buffer() {
        let dev = RwBytes::new(vec![0u8; 4]);
        assert!(dev.write_at(2, &[1, 2, 3, 4]).is_err());
        assert_eq!(dev.size_bytes(), 4, "the device did not grow");
    }

    #[test]
    fn reads_and_writes_inside_the_buffer_round_trip() {
        let dev = RwBytes::new(vec![0u8; 16]);
        dev.write_at(4, &[1, 2, 3, 4]).unwrap();
        let mut buf = [0u8; 4];
        dev.read_at(4, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4]);
    }
}
