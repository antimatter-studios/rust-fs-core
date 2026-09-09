//! Every `pub mod` in `lib.rs` appears in the README.
//!
//! # Why this is a test rather than a habit
//!
//! The README has drifted twice. `docs/human-code-status.md` records the
//! first as H3 — "the README omitted four modules and listed two shipped
//! types as unimplemented" — corrected on 2026-08-30. `counting_device.rs`
//! landed on 2026-09-06 without touching the README, so six days later the
//! same defect was back: a `pub mod`, re-exported from `lib.rs`, that the
//! README did not mention in any spelling.
//!
//! The cost is specific rather than tidiness. A reader takes the README as
//! the inventory of what the crate provides, and `counting_device.rs` exists
//! precisely so that every driver is measured by one instrument instead of
//! each writing its own. A reader who concludes it does not exist writes
//! their own, which is the outcome the module was created to prevent.
//!
//! # Why the parse is asserted before the thing it feeds
//!
//! A scan over a list it failed to build passes for the wrong reason. If the
//! `pub mod` pattern ever stops matching — a formatting change, an attribute
//! moved onto the line — the loop below runs zero times and reports success,
//! which is the exact shape of defect this file exists to catch. So the parse
//! has to find a plausible number of modules, and one named here by hand,
//! before its output is trusted.

use std::path::Path;

/// A module `lib.rs` certainly declares. If the parse cannot find this, it
/// is not reading what it thinks it is reading.
const KNOWN_MODULE: &str = "block";

/// The crate has had ten public modules for some time. A parse returning
/// fewer has more likely broken than the crate has shrunk — and if the crate
/// really did shrink, this number is meant to be edited on purpose.
const AT_LEAST: usize = 8;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The `pub mod` names declared in `lib.rs`.
///
/// `#[cfg(test)]` modules are not `pub mod` and so are not collected —
/// `test_device.rs` is deliberately outside this check, since it is not
/// public API and has no business in a reader's inventory.
fn public_modules(lib_rs: &str) -> Vec<String> {
    lib_rs
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub mod "))
        .filter_map(|rest| rest.strip_suffix(';'))
        .map(|name| name.trim().to_owned())
        .collect()
}

/// Each public module's FILE appears in the README.
///
/// The file name rather than the bare module name, deliberately. Half these
/// names are ordinary words — `block`, `error`, `slice`, `stream` — and
/// looking for them as substrings would be satisfied by the prose at the top
/// of the README, so a module could vanish from the layout tree and still
/// pass. Requiring `<name>.rs` pins the tree, which is the part that claims
/// to be an inventory.
#[test]
fn every_public_module_appears_in_the_readme() {
    let lib = read("src/lib.rs");
    let readme = read("README.md");
    let mods = public_modules(&lib);

    assert!(
        mods.len() >= AT_LEAST,
        "parsed only {} public modules out of src/lib.rs, expected at least \
         {AT_LEAST}. The `pub mod` pattern has probably stopped matching, and \
         an empty list would make the check below pass over nothing: {mods:?}",
        mods.len()
    );
    assert!(
        mods.iter().any(|m| m == KNOWN_MODULE),
        "the parse did not find `{KNOWN_MODULE}`, which src/lib.rs declares, \
         so it is not reading what it thinks it is: {mods:?}"
    );

    let missing: Vec<String> = mods
        .iter()
        .filter(|m| !readme.contains(&format!("{m}.rs")))
        .cloned()
        .collect();

    assert!(
        missing.is_empty(),
        "public modules missing from README.md: {missing:?}\n\
         Every `pub mod` in src/lib.rs is public API, and a reader who takes \
         the README as the inventory of this crate will go and write their \
         own version of whatever is not listed. Add each to the Layout tree, \
         and to `What it gives you` if it names a type worth knowing about."
    );
}
