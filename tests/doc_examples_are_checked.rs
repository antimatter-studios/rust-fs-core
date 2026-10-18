//! No documentation example in `src/` is fenced `ignore`.
//!
//! # Why this is a test rather than a habit
//!
//! An `ignore` fence is a doctest that never compiles. `cargo test` counts it
//! as ignored rather than failed, so the example can rot — name a type that
//! moved, call a method that was renamed, lean on a crate the library does not
//! depend on — while every run stays green. The streaming example in
//! `src/stream.rs` did exactly that: it hashed with `sha2`, which the library
//! has never depended on, and the unit tier reported it as one ignored test on
//! every run (#319). An ignored test reads like a passing one; that is the
//! "nothing skips" rule, applied to the examples a reader copies first.
//!
//! An example that cannot run on its own may still be fenced `no_run`: it is
//! compiled, so it cannot drift from the API it shows.
//!
//! # Why the scan is asserted before its verdict
//!
//! A scan that finds nothing passes for the wrong reason. If the walk stops
//! reaching the sources, or the fence pattern stops matching, the check runs
//! over zero examples and reports success. So it must first find the
//! `no_run` example in `src/cli.rs` and a plausible number of fences overall.

use std::path::{Path, PathBuf};

/// A fence that certainly exists. If the scan cannot see this one, it is not
/// reading what it thinks it is reading.
const KNOWN_FENCE: (&str, &str) = ("cli.rs", "```no_run");

/// The crate has carried more than this many documentation fences for some
/// time. Fewer means the scan broke rather than the docs shrank.
const AT_LEAST_FENCES: usize = 10;

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The fence opener on a doc-comment line, if the line holds one.
fn doc_fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let body = t.strip_prefix("//!").or_else(|| t.strip_prefix("///"))?;
    let body = body.trim_start();
    body.starts_with("```").then_some(body)
}

#[test]
fn no_documentation_example_is_fenced_ignore() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);

    let mut fences = 0;
    let mut saw_known = false;
    let mut ignored = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        for (n, line) in text.lines().enumerate() {
            let Some(fence) = doc_fence(line) else {
                continue;
            };
            fences += 1;
            if name == KNOWN_FENCE.0 && fence.starts_with(KNOWN_FENCE.1) {
                saw_known = true;
            }
            let info = fence.trim_start_matches('`');
            if info.split(',').any(|tag| tag.trim() == "ignore") {
                let rel = file.strip_prefix(&src).unwrap_or(file);
                ignored.push(format!("src/{}:{}", rel.display(), n + 1));
            }
        }
    }

    assert!(
        saw_known,
        "the scan did not find the `{}` fence in src/{}; it is not reading the sources",
        KNOWN_FENCE.1, KNOWN_FENCE.0
    );
    assert!(
        fences >= AT_LEAST_FENCES,
        "the scan found {fences} documentation fences, expected at least {AT_LEAST_FENCES}"
    );
    assert!(
        ignored.is_empty(),
        "documentation examples fenced `ignore` never compile; make them run, or fence \
         them `no_run` so they are at least compiled: {ignored:?}"
    );
}
