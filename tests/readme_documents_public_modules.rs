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
        .filter_map(declared_module)
        .collect()
}

/// The module name one line declares, if it declares one.
///
/// THE LINE DOES NOT HAVE TO BE EXACTLY `pub mod name;`. It used to:
/// the parse was `strip_prefix("pub mod ")` then `strip_suffix(';')`,
/// so a trailing comment, a `cfg` attribute on the same line, or an
/// inline body all failed to match. A module declared any of those ways
/// was never collected, and therefore never checked against the README
/// at all -- the check could only fail for a module its own parser
/// recognised, which is the defect this file was written to catch,
/// living inside the detector.
///
/// `AT_LEAST` would not have caught it either: at ten real modules
/// against a floor of eight, two could vanish this way in silence.
fn declared_module(line: &str) -> Option<String> {
    // NO EXPLICIT GUARD AGAINST A COMMENTED-OUT DECLARATION, and that
    // is deliberate rather than an omission. One was written here and
    // mutation testing showed it could not change any outcome: the
    // `strip_prefix("pub mod ")` below runs after `trim_start()`, so a
    // line beginning `//` fails it already, and the attribute branch
    // cannot produce a `rest` that begins with `pub mod ` from a line
    // that began with `//`. A guard that cannot fire is worse than none
    // -- it reassures the next reader that the case is handled here,
    // when it is really handled four lines down.
    //
    // `the_parser_does_not_invent_modules_that_are_not_public` pins the
    // behaviour regardless, so moving the trim or loosening the prefix
    // match fails a test rather than silently accepting comments.
    //
    // An attribute may open the line: `#[cfg(feature = "x")] pub mod foo;`.
    let rest = match line.rfind("] ") {
        Some(i) if line.starts_with("#[") => &line[i + 2..],
        _ => line,
    };
    // `pub mod` only. `pub(crate) mod` and a bare `mod` are not public
    // API and have no business in a reader's inventory.
    let rest = rest.trim_start().strip_prefix("pub mod ")?;
    // The name ends at whatever follows it -- `;`, `{`, or the space
    // before a comment.
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The fenced block under `## Layout`.
///
/// THE WHOLE README USED TO COUNT, which made the check satisfiable by
/// a filename appearing anywhere at all -- a "planned additions" note,
/// a sentence of prose, a link. This file's own message tells the
/// reader to add the module "to the Layout tree", because the tree is
/// the part that claims to be an inventory; searching outside it
/// accepts evidence the message does not ask for.
fn layout_block(readme: &str) -> String {
    let mut out = String::new();
    let mut in_layout = false;
    let mut in_fence = false;
    for line in readme.lines() {
        let t = line.trim();
        if t.starts_with("## ") {
            // Any new section ends Layout, fenced or not.
            in_layout = t.eq_ignore_ascii_case("## Layout");
            continue;
        }
        if !in_layout {
            continue;
        }
        if t.starts_with("```") {
            if in_fence {
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
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

    // The layout block is asserted before it is searched, for the same
    // reason the module list is: a search over a block the extractor
    // failed to find reports every module missing, or -- if it returned
    // the whole file -- reports none, and both are answers to a
    // question nobody asked.
    let layout = layout_block(&readme);
    assert!(
        layout.contains(&format!("{KNOWN_MODULE}.rs")),
        "could not find `{KNOWN_MODULE}.rs` in the fenced block under `## Layout`, \
         so the Layout tree was not extracted and the check below would be \
         searching the wrong text. Extracted {} bytes: {layout:?}",
        layout.len()
    );

    let missing: Vec<String> = mods
        .iter()
        .filter(|m| !layout.contains(&format!("{m}.rs")))
        .cloned()
        .collect();

    assert!(
        missing.is_empty(),
        "public modules missing from README.md: {missing:?}\n\
         Every `pub mod` in src/lib.rs is public API, and a reader who takes \
         the README as the inventory of this crate will go and write their \
         own version of whatever is not listed. Add each to the Layout tree, \
         and to `What it gives you` if it names a type worth knowing about.\n\
         Note this searches the Layout block ONLY -- naming the file elsewhere \
         in the README does not satisfy it."
    );
}

/// THE PARSER SEES A MODULE HOWEVER ITS LINE IS WRITTEN.
///
/// This is the regression case for the first gap, and it has to be a
/// test on the parser rather than on the crate: every real `pub mod`
/// line in `lib.rs` is currently the plain form, so the tree cannot
/// discriminate. A module declared any other way used to be dropped on
/// the floor and never checked against the README at all — the check
/// silently narrowing to whatever its own parser happened to match.
#[test]
fn the_parser_sees_a_module_however_its_line_is_written() {
    let src = r#"
pub mod plain;
pub mod trailing_comment; // not for outside use
#[cfg(feature = "extra")] pub mod behind_an_attribute;
pub mod inline_body { pub fn f() {} }
"#;
    assert_eq!(
        public_modules(src),
        vec![
            "plain",
            "trailing_comment",
            "behind_an_attribute",
            "inline_body"
        ],
        "a public module was dropped because of how its line is written, which is \
         how one skips the README check entirely"
    );
}

/// AND DOES NOT INVENT ONE.
///
/// The looser parse must not start matching things that are not public
/// module declarations — a commented-out line especially, since the
/// whole parse is substring work and `// pub mod ghost;` contains the
/// pattern exactly.
#[test]
fn the_parser_does_not_invent_modules_that_are_not_public() {
    let src = r#"
// pub mod commented_out;
/// pub mod in_a_doc_comment;
mod private_one;
pub(crate) mod crate_only;
use std::pub_mod_lookalike;
"#;
    assert_eq!(
        public_modules(src),
        Vec::<String>::new(),
        "the parser claimed a public module where lib.rs declares none, which would \
         demand a README entry for something that does not exist"
    );
}

/// ONLY THE LAYOUT BLOCK COUNTS AS THE INVENTORY.
///
/// The regression case for the second gap. The check used to search the
/// whole README, so a filename mentioned in prose, in a planned-work
/// note, or in a link satisfied it while the module never appeared in
/// the tree the failure message tells you to add it to.
#[test]
fn only_the_layout_block_counts_as_the_inventory() {
    let readme = r#"# am-fs-core

Prose that happens to mention ghost.rs in passing.

## Layout

```
src/
  real.rs        does a real thing
```

## Planned additions

- one day, maybe, planned.rs
"#;
    let block = layout_block(readme);
    assert!(
        block.contains("real.rs"),
        "the Layout tree's own contents were not extracted: {block:?}"
    );
    assert!(
        !block.contains("ghost.rs"),
        "prose above the Layout section satisfied the inventory check: {block:?}"
    );
    assert!(
        !block.contains("planned.rs"),
        "a later section satisfied the inventory check, so a module could be \
         'documented' by a note saying it does not exist yet: {block:?}"
    );
}

/// A LAYOUT SECTION THAT CANNOT BE FOUND IS EMPTY, NOT THE WHOLE FILE.
///
/// The extractor's failure mode has to be the loud one. Returning the
/// whole README on a miss would restore exactly the gap this replaces,
/// and it would do it silently.
#[test]
fn a_missing_layout_section_yields_nothing_rather_than_everything() {
    let readme = "# am-fs-core\n\nNo layout section here, but block.rs is named.\n";
    assert!(
        layout_block(readme).is_empty(),
        "a README with no Layout section produced a non-empty block, so the check \
         would search text that is not an inventory"
    );
}
