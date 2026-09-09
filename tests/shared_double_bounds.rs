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

/// THE ONE SHAPE WHERE "AVAILABLE" AND "COPIED" DIFFER, WHICH NOTHING
/// HERE COVERED.
///
/// Every case above starts at or past the end — `u64::MAX`,
/// `u64::MAX - 7`, `1 << 40`, `9` against eight bytes — where
/// `len - offset` saturates to 0 and the two readings of `got` agree by
/// accident. A read that BEGINS INSIDE the buffer and only overruns at
/// the far end is the case that tells them apart, and there was no such
/// test, so the helper reported `got: 8` for a read that copied nothing
/// and the whole suite stayed green.
///
/// `error.rs` is explicit that `got` counts what was placed in the
/// caller's buffer, and that a device which refuses rather than
/// copying a prefix reports 0. This double refuses: `read_into` returns
/// through `?` before `copy_from_slice`. The assertion on the untouched
/// buffer and the assertion on `got` are the same claim said twice, and
/// the point is that they used to disagree.
///
/// The offsets are swept rather than singular because 0 is the special
/// case that would pass a `got: offset` implementation too.
#[test]
fn a_read_beginning_inside_the_buffer_that_overruns_it_copies_nothing() {
    let bytes = [0xABu8; 8];
    for (offset, available) in [(0u64, 8usize), (1, 7), (7, 1)] {
        let mut buf = [0u8; 16];
        match common::read_into(&bytes, offset, &mut buf).expect_err("overruns the end") {
            Error::ShortRead {
                offset: o,
                want,
                got,
            } => {
                assert_eq!((o, want), (offset, 16));
                assert_eq!(
                    got, 0,
                    "{available} bytes were available from {offset}, and none of them \
                     were copied -- got counts the buffer, not the source"
                );
            }
            other => panic!("expected ShortRead at {offset}, got {other:?}"),
        }
        assert_eq!(buf, [0u8; 16], "a refused read leaves the buffer alone");
    }
}

/// And the write half, which shares the same arithmetic and had the
/// same blind spot.
#[test]
fn a_write_beginning_inside_the_buffer_that_overruns_it_writes_nothing() {
    for offset in [0u64, 1, 7] {
        let mut bytes = [0u8; 8];
        match common::write_from(&mut bytes, offset, &[1u8; 16]).expect_err("overruns the end") {
            Error::ShortRead {
                offset: o,
                want,
                got,
            } => {
                assert_eq!((o, want), (offset, 16));
                assert_eq!(got, 0, "a refused write transfers nothing");
            }
            other => panic!("expected ShortRead at {offset}, got {other:?}"),
        }
        assert_eq!(bytes, [0u8; 8], "no refused write reached the buffer");
    }
}
