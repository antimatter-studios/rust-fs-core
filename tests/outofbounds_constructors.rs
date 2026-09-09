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
/// Anything below a `#[cfg(test)]` is skipped: a test building the
/// value to check its formatting is not a place the crate reports one
/// from.
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
        let production = match text.find("#[cfg(test)]") {
            Some(at) => &text[..at],
            None => &text[..],
        };
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
