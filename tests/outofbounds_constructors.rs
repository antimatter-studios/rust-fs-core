//! Where `Error::OutOfBounds` is built, pinned against the source.
//!
//! # Why an inventory rather than a prose check
//!
//! `error.rs` and `ffi.rs` both describe this variant by enumerating
//! who constructs it, and both said "exactly one place" for as long as
//! it took someone to add the second -- a write bound on
//! `FileDevice::write_at`. Nothing went red, because a doc comment
//! cannot.
//!
//! Prose is not machine-checkable past a point, and a test that
//! searched those comments for the right words would mostly be a test
//! of the words. What IS checkable is the fact the prose is about: the
//! set of places the variant is constructed. When that set changes this
//! fails, which is the moment someone has to look at the paragraphs
//! again. It does not verify that they were then written correctly.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The files this crate constructs `Error::OutOfBounds` in, and what
/// each one is called in the documentation that enumerates them.
const DOCUMENTED: &[(&str, &str)] = &[
    ("file_device.rs", "FileDevice::write_at"),
    ("slice.rs", "OwnedRwSlice::write_at"),
];

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Whether a line BUILDS the variant, as opposed to matching it.
///
/// A construction opens a struct literal and leaves the fields to the
/// lines below, so it ends in `{`; a pattern carries its fields and its
/// `=>` on the one line. Crude, and checked in both directions by the
/// tests below rather than taken on trust: `a_pattern_is_not_a_
/// construction` is the half that would otherwise rot quietly.
fn is_construction(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") {
        return false;
    }
    t.contains("Error::OutOfBounds {") && !t.contains("=>")
}

/// The part of a source file that is not the test module.
///
/// # WHY NOT "EVERYTHING BEFORE THE FIRST `#[cfg(test)]`", WHICH IS
/// WHAT THIS WAS
///
/// That attribute does not belong to the test module alone. It sits on
/// individual items and struct FIELDS too, and `src/file_device.rs`
/// grew three of them — the lock-arrival witness rust-fs-core#104
/// needed — around 400 lines above `FileDevice::write_at`. Cutting at
/// the first one dropped `write_at` out of the scan, and this file
/// then failed saying `["file_device.rs"]` was documented as
/// constructing `Error::OutOfBounds` "and no longer does". It still
/// did; the scan had stopped early. The count assertion agreed,
/// reporting 1 site where the paragraph says two.
///
/// It failed loudly this time, which was luck: the same cut hides an
/// UNDOCUMENTED constructor just as readily, and that direction is
/// silent. `src/caching_device.rs` (first `#[cfg(test)]` at an
/// item, line 291, test module at 922) and `src/lib.rs` were already
/// being cut early on main; neither happens to construct the variant,
/// so the hole was latent rather than harmless.
///
/// So the boundary looked for is the test MODULE: an unindented
/// `#[cfg(test)]` whose next line opens an inline `mod`. A
/// `#[cfg(test)] mod name;` DECLARATION is deliberately not a
/// boundary — `src/lib.rs` has one on line 11 with production code
/// under it.
///
/// Line scanning rather than parsing, because there is no Rust parser
/// in this crate's dev-dependencies and adding one to find a `mod` is
/// not a trade worth making. The two tests below pin both directions.
fn production_half(text: &str) -> &str {
    /// A line WITH its terminator, so its length is its span in `text`.
    ///
    /// This was `lines()` plus `offset += line.len() + 1`, which
    /// assumes a one-byte line ending. `lines()` strips a trailing
    /// `\r`, so on a CRLF checkout the length excludes it while the
    /// `+ 1` counts only the `\n`, and the cut point falls one byte
    /// further behind the truth with every preceding line. The
    /// production half is then TRUNCATED — the scan covers less source
    /// than it claims to and still passes, which is the defect this
    /// file exists to catch, in the file catching it. A drifted offset
    /// can also land inside a multi-byte character, where slicing
    /// panics.
    ///
    /// `split_inclusive` removes the arithmetic rather than correcting
    /// it: the offsets are sums of real slice lengths, so they are
    /// exact for either line ending and always on a character
    /// boundary. Nothing here depends on which one the checkout used.
    fn strip_ending(raw: &str) -> &str {
        raw.trim_end_matches(['\n', '\r'])
    }

    let mut offset = 0usize;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(raw) = lines.next() {
        let start = offset;
        offset += raw.len();
        if strip_ending(raw) != "#[cfg(test)]" {
            continue;
        }
        let next = lines.peek().copied().map(strip_ending).unwrap_or("");
        let opens_a_module = (next.starts_with("mod ")
            || next.starts_with("pub mod ")
            || next.starts_with("pub(crate) mod "))
            && next.ends_with('{');
        if opens_a_module {
            return &text[..start];
        }
    }
    text
}

/// A `#[cfg(test)]` ON AN ITEM IS NOT THE TEST MODULE, and the scan
/// must not stop at one. This is the regression `file_device.rs`
/// produced: the production constructor sits below a test-only field.
#[test]
fn a_cfg_test_item_does_not_end_the_production_half() {
    let text = "\
struct D {
    #[cfg(test)]
    arrivals: LockArrivals,
}

#[cfg(test)]
struct LockArrivals;

fn write_at() {
    return Err(Error::OutOfBounds {
        offset,
    });
}

#[cfg(test)]
mod tests {
    fn t() {
        let _ = Error::OutOfBounds {
            offset: 0,
        };
    }
}
";
    let production = production_half(text);
    assert!(
        production.contains("fn write_at()"),
        "the scan stopped at a test-only item and lost the production constructor"
    );
    assert!(
        !production.contains("mod tests"),
        "the test module must still be excluded"
    );
    assert_eq!(
        production.matches("Error::OutOfBounds {").count(),
        1,
        "exactly the production constructor, and not the one inside mod tests"
    );
}

/// THE CUT IS A BYTE OFFSET IN THE TEXT, NOT A COUNT OF LINES.
///
/// The first version added `line.len() + 1` per line. `lines()` strips
/// a trailing `\r`, so on CRLF that is one byte short per line and the
/// returned slice ends progressively before the boundary it names —
/// the production half silently truncated, the scan covering less than
/// it claims, and passing. The runner checks out LF, so nothing was
/// firing; that is the reason it needs a fixture rather than a
/// `.gitattributes`, which would hide it and leave the function wrong
/// for any caller that passes CRLF text of its own.
///
/// The assertion is on the CUT rather than on what the slice contains:
/// the remainder must begin exactly at the test module's attribute. A
/// containment check passes while the offset is a few bytes out, which
/// is the whole failure mode.
///
/// The em dash is deliberate. A drifted offset that lands inside a
/// multi-byte character does not truncate, it PANICS, and this
/// module's real sources are full of them.
#[test]
fn the_boundary_is_a_byte_offset_and_survives_either_line_ending() {
    const LINES: &[&str] = &[
        "// a comment with an em dash — as the real sources have",
        "#[cfg(test)]",
        "struct Witness;",
        "",
        "fn write_at() {",
        "    return Err(Error::OutOfBounds {",
        "        offset,",
        "    });",
        "}",
        "",
        "#[cfg(test)]",
        "mod tests {",
        "}",
        "",
    ];

    for (what, ending) in [("LF", "\n"), ("CRLF", "\r\n")] {
        let text = LINES.join(ending);
        let production = production_half(&text);
        assert!(
            production.contains("fn write_at()"),
            "{what}: the production constructor fell outside the production half"
        );
        let rest = &text[production.len()..];
        assert!(
            rest.starts_with("#[cfg(test)]"),
            "{what}: the cut landed {} bytes in, and the text there begins {:?} \
             rather than at the test module's attribute. An offset computed from a \
             line count is wrong by one byte per line whenever the ending is two.",
            production.len(),
            &rest[..rest.len().min(24)]
        );
    }
}

/// AND THE FILE ON DISK, not only the sample above. A hand-written
/// fixture can drift from the real thing; this is the assertion that
/// would have caught the regression at the moment it was introduced.
#[test]
fn the_real_file_device_keeps_its_constructor_in_the_production_half() {
    let text =
        std::fs::read_to_string(src_dir().join("file_device.rs")).expect("read src/file_device.rs");
    assert!(
        text.contains("    #[cfg(test)]"),
        "control: this asserts something only while file_device.rs still carries a \
         test-only item above write_at. If that has gone, so has the regression, \
         and this test is measuring nothing"
    );
    let production = production_half(&text);
    assert!(
        production.contains("fn write_at("),
        "FileDevice::write_at fell outside the production half of its own file"
    );
    assert!(
        !production.contains("fn rw_image("),
        "the test module leaked into the production half"
    );
}

/// Every place this crate constructs it, as `file:line`.
///
/// SITES, NOT FILES, and the difference is the defect this returned
/// for. `error.rs` says "exactly two PLACES", naming two call sites
/// that happen to sit in two files. Counting files makes those numbers
/// agree by coincidence, and a third constructor added to a file
/// already in the set leaves the paragraph wrong and the test green --
/// which is the shape of the original defect, reproduced by the check
/// written to catch it.
///
/// Anything below the test module is skipped: a test building the
/// value to check its formatting is not a place the crate reports one
/// from. Which line that is, and why it is not simply the first
/// `#[cfg(test)]`, is [`production_half`].
fn constructing_sites() -> Vec<String> {
    let mut found = Vec::new();
    let dir = src_dir();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("src/ must be readable")
        .map(|e| e.expect("a readable dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a readable source file");
        let production = production_half(&text);
        let name = path
            .file_name()
            .expect("a named file")
            .to_string_lossy()
            .into_owned();
        for (i, line) in production.lines().enumerate() {
            if is_construction(line) {
                found.push(format!("{name}:{}", i + 1));
            }
        }
    }
    found
}

/// The files those sites live in.
fn constructing_files() -> BTreeSet<String> {
    constructing_sites()
        .into_iter()
        .filter_map(|site| site.split(':').next().map(str::to_string))
        .collect()
}

#[test]
fn the_documented_constructors_are_the_ones_in_the_source() {
    let found = constructing_files();
    assert!(
        !found.is_empty(),
        "control: the scan found no constructor at all, so it is measuring nothing \
         and every assertion below would pass vacuously"
    );
    let documented: BTreeSet<String> = DOCUMENTED.iter().map(|(f, _)| f.to_string()).collect();

    let undocumented: Vec<_> = found.difference(&documented).collect();
    assert!(
        undocumented.is_empty(),
        "{undocumented:?} construct Error::OutOfBounds and are not in this table. \
         Add them here AND to the paragraphs that enumerate this variant -- \
         src/error.rs's `**This crate constructs it in exactly ... places**` and \
         src/ffi.rs's OutOfBounds doc -- which is the thing this test exists to \
         force someone to reread."
    );
    let missing: Vec<_> = documented.difference(&found).collect();
    assert!(
        missing.is_empty(),
        "{missing:?} are documented as constructing Error::OutOfBounds and no longer \
         do. A stale entry here is the same defect as a missing one, pointing the \
         other way."
    );
}

/// The half of `is_construction` that has nothing else watching it: a
/// `match` arm names the variant without building one, and counting
/// those would make every file that handles the error look like a file
/// that reports it -- including `ffi.rs` and `stream.rs`, which only
/// translate it.
#[test]
fn a_pattern_is_not_a_construction() {
    assert!(is_construction(
        "            return Err(Error::OutOfBounds {"
    ));
    assert!(is_construction("        .ok_or(Error::OutOfBounds {"));
    assert!(!is_construction(
        "            Error::OutOfBounds { offset, len, size } => {"
    ));
    assert!(!is_construction(
        "            Err(Error::OutOfBounds { .. }) => FsCoreErrorCode::OutOfBounds,"
    ));
    assert!(!is_construction(
        "/// is [`Error::OutOfBounds`] from this slice"
    ));
    assert!(!is_construction(
        "// Error::OutOfBounds { offset, len, size }"
    ));
}

/// The paragraph states a COUNT, and the count is what was wrong.
///
/// `error.rs` said "exactly one place" for as long as it took someone
/// to add the second, and nothing went red. The inventory above forces
/// a reread when the set changes; it does not enforce the result -- the
/// next person can satisfy it by adding a row to `DOCUMENTED` and never
/// touching the prose, which is the same shape as the defect itself.
/// This is the half that makes that red.
///
/// A NAME cannot be checked here, and that is worth recording so it is
/// not attempted again: the doc writes the constructor as
/// ``[`FileDevice`]'s `write_at` `` rather than as a qualified path, so
/// `FileDevice::write_at` appears zero times in `error.rs` both before
/// and after the correction. The count is the one part of the sentence
/// that is a fact about the source.
#[test]
fn the_documented_count_matches_the_source() {
    /// How the paragraph spells its number. It is prose, so it is a
    /// word rather than a digit.
    const WORDS: &[(&str, usize)] = &[
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
    ];
    const MARKER: &str = "constructs it in exactly ";

    let text = std::fs::read_to_string(src_dir().join("error.rs")).expect("read src/error.rs");
    // Loud rather than silent. A reworded paragraph must fail here, not
    // quietly leave this test checking nothing -- the same reason the
    // inventory asserts `!found.is_empty()` before comparing.
    let at = text
        .find(MARKER)
        .expect("src/error.rs must still enumerate where OutOfBounds is constructed");
    let word = text[at + MARKER.len()..]
        .split_whitespace()
        .next()
        .expect("a count word after the marker");
    let stated = WORDS
        .iter()
        .find(|(w, _)| *w == word)
        .map(|(_, n)| *n)
        .unwrap_or_else(|| panic!("unrecognised count word {word:?} in src/error.rs"));

    let sites = constructing_sites();
    let actual = sites.len();
    assert_eq!(
        stated, actual,
        "src/error.rs says OutOfBounds is constructed in exactly {word} place(s), and \
         the source constructs it in {actual}: {sites:?}. The paragraph and the code \
         disagree, which is the defect this file exists to catch."
    );
}

/// SITES AND FILES ARE NOT THE SAME NUMBER, and pinning the paragraph
/// against the wrong one is what this test file was returned for. They
/// happen to be equal today -- two constructors in two files -- so
/// nothing above can tell them apart, and this says which one the count
/// is asserted on.
#[test]
fn the_count_is_of_sites_and_not_of_files() {
    let sites = constructing_sites();
    let files = constructing_files();
    assert!(
        sites.len() >= files.len(),
        "every file in the set has at least one site in it: {sites:?} vs {files:?}"
    );
    for site in &sites {
        assert!(
            site.contains(':'),
            "a site is `file:line`, so that two constructors in one file are two \
             entries rather than one: {site:?}"
        );
    }
}
