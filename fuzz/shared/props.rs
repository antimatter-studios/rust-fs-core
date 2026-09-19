// Shared by both tiers, included textually rather than depended on.
//
// `tests/fuzz_decoders.rs` and `fuzz/src/lib.rs` both `include!` this
// file. A crate dependency would have been tidier, but the fuzz crate
// depends on `libfuzzer-sys`, which builds libFuzzer's C++ runtime, and
// making the gate depend on the fuzz crate would drag that into every
// pull request build on the stable toolchain.
//
// FUZZING THIS CRATE IS NOT LIKE FUZZING THE OTHERS. There are no
// structures here to mutate: nothing in rust-fs-core parses a
// filesystem. What it does is arithmetic on offsets and lengths that
// ultimately came from an image -- a partition start, an extent offset,
// a declared device size -- and the release profile has
// `overflow-checks` off. "Arithmetic that wrapped in the release
// profile and answered a read inside a slice's own declared length with
// somebody else's bytes" is on the list of things the 2026-09-06
// hardening wave fixed by hand.
//
// So the input is read as GEOMETRY rather than as a structure, and each
// target asserts a property rather than merely surviving. A target that
// only checked for panics would pass on a slice that answered the wrong
// bytes, which is the defect that actually happened.

// `Arc` is fully qualified below rather than imported: this file is
// `include!`d into modules that already import it, and a duplicate
// `use` is a hard error.
use fs_core::{BlockRead, CachingDevice, OwnedSlice, Result as CoreResult, SliceReader};

/// The parent device every geometry case is measured against.
///
/// Filled with a position-dependent pattern rather than zeros or a
/// constant: a slice that reads from the wrong offset returns bytes
/// that are *valid* but belong somewhere else, and only a pattern that
/// differs per position can tell the two apart. That is precisely the
/// defect this crate had.
pub const PARENT_LEN: u64 = 4096;

pub fn parent_byte(at: u64) -> u8 {
    // A cheap position hash: adjacent bytes differ, and so do bytes a
    // power of two apart, which a shift-based bug would otherwise line
    // up.
    (at.wrapping_mul(2_654_435_761) >> 13) as u8
}

pub struct Parent(pub Vec<u8>);

impl Parent {
    pub fn new() -> Self {
        Parent((0..PARENT_LEN).map(parent_byte).collect())
    }
}

impl Default for Parent {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockRead for Parent {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> CoreResult<()> {
        // A ZERO-LENGTH READ SUCCEEDS AT ANY OFFSET, including one past
        // the end. That is what this crate's real devices do -- measured
        // on both `FileDevice` and `CachingDevice` -- and a stand-in
        // that refused it would manufacture a disagreement between the
        // cache and the device that no real pair has. The first version
        // of this harness did exactly that, and the property duly
        // reported it.
        if buf.is_empty() {
            return Ok(());
        }
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(buf.len());
        if end > self.0.len() {
            return Err(fs_core::Error::ShortRead {
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

/// Four numbers read out of the fuzzer's bytes: where a slice starts,
/// how long it claims to be, and the offset and length of a read into
/// it.
///
/// Taken little-endian from the first 32 bytes and then folded into
/// ranges that straddle the parent's end. Left unfolded, almost every
/// case would be a wild offset the first bounds check rejects, and the
/// interesting ones -- a read ending exactly at the boundary, or one
/// byte past it -- would almost never come up.
pub struct Geometry {
    pub start: u64,
    pub length: u64,
    pub offset: u64,
    pub read_len: usize,
}

pub fn geometry_from(data: &[u8]) -> Geometry {
    let word = |i: usize| -> u64 {
        let mut b = [0u8; 8];
        for (k, slot) in b.iter_mut().enumerate() {
            *slot = data.get(i * 8 + k).copied().unwrap_or(0);
        }
        u64::from_le_bytes(b)
    };

    // Most of the time, land near the edges; occasionally go wild, so
    // the overflow paths are still reached.
    let near = |v: u64| -> u64 {
        if v.is_multiple_of(8) {
            v
        } else {
            v % (PARENT_LEN + 2)
        }
    };

    Geometry {
        start: near(word(0)),
        length: near(word(1)),
        offset: near(word(2)),
        read_len: (word(3) % (PARENT_LEN + 2)) as usize,
    }
}

/// A slice must answer a read inside it with the parent's bytes from
/// `start + offset`, and must refuse anything else.
///
/// This is the property, not "it did not panic". A slice that answered
/// with somebody else's bytes would pass a panic check and fail here.
pub fn check_slice(geom: &Geometry) {
    let parent = Parent::new();
    let mut buf = vec![0u8; geom.read_len];

    let borrowed = SliceReader::new(&parent, geom.start, geom.length);
    let result = borrowed.read_at(geom.offset, &mut buf);

    // Whether the read should have been answered at all.
    //
    // A ZERO-LENGTH READ SUCCEEDS ANYWHERE, including at an offset past
    // the end of everything. That is this crate's contract -- measured
    // on `FileDevice` and `CachingDevice`, which both accept one -- and
    // it is stated once here rather than patched into each arm below,
    // because the two arms have to agree about it or the property
    // contradicts itself.
    // THE DECLARED LENGTH IS NOT THE EFFECTIVE ONE. A slice cannot
    // report more device than its parent holds, so a slice that claims
    // more is clamped to what is actually there -- the rule
    // `slice_rw_length_is_clamped_and_a_write_past_it_does_not_grow_the_image`
    // is named for. A property that compared against the declared
    // length would report every over-long slice as a bug.
    let effective_length = geom.length.min(PARENT_LEN.saturating_sub(geom.start));
    let fits_slice = geom
        .offset
        .checked_add(geom.read_len as u64)
        .is_some_and(|end| end <= effective_length);
    let absolute_end = geom
        .start
        .checked_add(geom.offset)
        .and_then(|at| at.checked_add(geom.read_len as u64));
    let fits_parent = absolute_end.is_some_and(|end| end <= PARENT_LEN);

    // TWO BOUNDS, AND ONLY ONE OF THEM IS WAIVED FOR AN EMPTY READ.
    //
    // The slice's own bound always applies: an offset past `length` is
    // outside the slice whether or not any bytes were asked for, and
    // `SliceReader` refuses it. The parent's bound is different, because
    // a zero-length read succeeds at ANY offset on a real device --
    // measured on `FileDevice` and `CachingDevice`, which both accept
    // one -- so a slice that sits entirely past the end of its parent
    // still answers an empty read, by passing it through.
    //
    // Getting this wrong in either direction manufactures a finding. I
    // had it wrong both ways before measuring: first refusing empty
    // reads in the stand-in parent, then waiving the slice's bound as
    // well as the parent's.
    let should_succeed = fits_slice && (geom.read_len == 0 || fits_parent);

    match result {
        Ok(()) => {
            assert!(
                should_succeed,
                "a slice at {}+{} answered a {}-byte read at {}, which does not fit in it",
                geom.start,
                geom.length,
                geom.read_len,
                geom.offset
            );
            let base = geom.start + geom.offset;
            for (i, got) in buf.iter().enumerate() {
                let want = parent_byte(base + i as u64);
                assert_eq!(
                    *got, want,
                    "a slice at {}+{} answered byte {i} of a read at {} with the byte from \
                     somewhere else",
                    geom.start, geom.length, geom.offset
                );
            }
        }
        Err(_) => {
            assert!(
                !should_succeed,
                "a slice at {}+{} refused a {}-byte read at {} that fits inside it",
                geom.start,
                geom.length,
                geom.read_len,
                geom.offset
            );
        }
    }

    // The owned variant must agree with the borrowed one. They are two
    // implementations of one contract, and the one thing worse than a
    // wrong answer is two different wrong answers.
    let owned = OwnedSlice::new(std::sync::Arc::new(Parent::new()), geom.start, geom.length);
    let mut other = vec![0u8; geom.read_len];
    let owned_result = owned.read_at(geom.offset, &mut other);
    assert_eq!(
        result.is_ok(),
        owned_result.is_ok(),
        "SliceReader and OwnedSlice disagreed about a {}-byte read at {} of a slice at {}+{}",
        geom.read_len,
        geom.offset,
        geom.start,
        geom.length
    );
    if result.is_ok() {
        assert_eq!(
            buf, other,
            "SliceReader and OwnedSlice returned different bytes for the same read"
        );
    }
}

/// A cache must answer exactly what the device underneath it would.
///
/// Every block size, including ones that do not divide the device, and
/// every read, including ones that straddle a block boundary or run off
/// the end.
pub fn check_cache(geom: &Geometry) {
    // A block size of zero is refused by the constructor rather than
    // dividing by zero; anything else is fair.
    let block_size = 1 + (geom.length % 1024);
    let capacity = (geom.start % 8) as usize;

    let direct = Parent::new();
    let cached = CachingDevice::read_only(std::sync::Arc::new(Parent::new()), block_size, capacity);

    let mut from_cache = vec![0u8; geom.read_len];
    let mut from_device = vec![0u8; geom.read_len];

    let cache_result = cached.read_at(geom.offset, &mut from_cache);
    let device_result = direct.read_at(geom.offset, &mut from_device);

    assert_eq!(
        cache_result.is_ok(),
        device_result.is_ok(),
        "the cache and the device disagreed about whether a {}-byte read at {} is allowed, \
         at a block size of {block_size}",
        geom.read_len,
        geom.offset
    );
    if device_result.is_ok() {
        assert_eq!(
            from_cache, from_device,
            "the cache returned different bytes from the device for a {}-byte read at {}, \
             at a block size of {block_size}",
            geom.read_len, geom.offset
        );
        // And again, so a second read of the same range comes from the
        // cache rather than the device. A cache that is right once and
        // wrong afterwards is the interesting failure.
        let mut again = vec![0u8; geom.read_len];
        cached.read_at(geom.offset, &mut again).expect("a read that succeeded once");
        assert_eq!(
            again, from_device,
            "the cache returned different bytes on the second read of the same range"
        );
    }
}
