//! The rule `tests/common/mod.rs` holds, pinned where the integration
//! doubles can see it.
//!
//! `src/test_device.rs::tests` pins the same three facts for the
//! `#[cfg(test)]` copy. Two copies of a rule need two tests, because a
//! test in one crate cannot observe the other drifting away from it.

use fs_core::error::Error;

mod common;

/// A WILD OFFSET IS A SHORT READ, NOT A CRASH. `offset as usize`
/// followed by `start + buf.len()` computes the end before the bounds
/// check that is supposed to refuse the range, so this input used to
/// panic with "attempt to add with overflow" inside the double.
#[test]
fn a_read_at_a_wild_offset_is_a_short_read_not_a_panic() {
    let bytes = [0xABu8; 8];
    for offset in [u64::MAX, u64::MAX - 7, 1 << 40, 9] {
        let mut buf = [0u8; 8];
        match common::read_into(&bytes, offset, &mut buf).expect_err("past the end") {
            Error::ShortRead {
                offset: o,
                want,
                got,
            } => {
                assert_eq!((o, want), (offset, 8));
                assert_eq!(got, 0, "nothing is available at {offset}");
            }
            other => panic!("expected ShortRead at {offset}, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 8], "a refused read leaves the buffer alone");
    }
}

/// An offset that fits, of a length that overflows when added to it.
/// The bounds check alone cannot catch this — the sum is what wraps.
#[test]
fn a_length_that_overflows_when_added_to_the_offset_is_refused() {
    let bytes = [0xABu8; 8];
    let mut buf = vec![0u8; 16];
    let offset = u64::MAX - 8;
    match common::read_into(&bytes, offset, &mut buf).expect_err("cannot fit") {
        Error::ShortRead { want, got, .. } => assert_eq!((want, got), (16, 0)),
        other => panic!("expected ShortRead, got {other:?}"),
    }
}

/// A device is not a `Vec`: a write past the end grows nothing, is
/// refused, and leaves the bytes that were there alone.
#[test]
fn a_write_at_a_wild_offset_is_refused_rather_than_panicking() {
    let mut bytes = [0u8; 8];
    for offset in [u64::MAX, u64::MAX - 7, 1 << 40, 9] {
        match common::write_from(&mut bytes, offset, &[1u8; 8]).expect_err("past the end") {
            Error::ShortRead {
                offset: o, want, ..
            } => assert_eq!((o, want), (offset, 8)),
            other => panic!("expected ShortRead at {offset}, got {other:?}"),
        }
    }
    assert_eq!(bytes, [0u8; 8], "no refused write reached the buffer");

    // And the ordinary write still lands.
    common::write_from(&mut bytes, 4, &[9u8; 4]).expect("inside the buffer");
    assert_eq!(bytes, [0, 0, 0, 0, 9, 9, 9, 9]);
}
