//! The family's release-tarball workflow is one reusable workflow, here.
//!
//! Every repository with command-line tools once carried its own
//! `package-cli` matrix and its own attest-and-attach job in `release.yml`,
//! and the copies drifted: one repository never got the jobs at all and
//! released without a tarball (#193). `.github/workflows/release-cli.yml`
//! is the one copy. A repository calls it from its `release.yml` with
//!
//! ```yaml
//! cli:
//!   needs: [test, publish]
//!   permissions:
//!     contents: write
//!     id-token: write
//!     attestations: write
//!   uses: antimatter-studios/rust-fs-core/.github/workflows/release-cli.yml@<sha> # vX.Y.Z
//!   with:
//!     core-ref: vX.Y.Z
//!     toolchain: 1.95.0
//! ```
//!
//! It runs only on a version tag, in another repository, so nothing here
//! would notice it losing a property until a release went out without it.
//! This file is that notice. The workflow is PARSED rather than scanned, so
//! a comment or a quoted string cannot satisfy a check meant for a real key
//! or step. `tests/scripts/test-write-jobs-pinned.sh` covers it too, as it
//! covers every workflow here.

use saphyr::{LoadableYamlNode, Yaml};
use std::collections::BTreeSet;
use std::path::Path;

const WORKFLOW: &str = ".github/workflows/release-cli.yml";

/// The action that signs the attestation, up to its `@`.
const ATTEST: &str = "actions/attest-build-provenance@";

/// How the packaging step runs core's script: in place, from the sibling
/// checkout the workflow clones beside the caller. No caller keeps a copy,
/// and none keeps the scripts/core.sh shim either (#212).
const PACKAGE: &str = "bash ../rust-fs-core/scripts/package-cli.sh";

/// The grants the attaching job needs, each at `write`: an OIDC token to
/// sign with, the attestation store, and the release to attach to.
const GRANTS: &[&str] = &["id-token", "attestations", "contents"];

/// The inputs a caller must pass: the rust-fs-core tag its `Cargo.toml`
/// pins (the path sibling the build needs), and its pinned toolchain.
const INPUTS: &[&str] = &["core-ref", "toolchain"];

/// The platforms every repository ships, one native runner each.
const LABELS: &[&str] = &["darwin-arm64", "linux-x86_64"];

/// The input a caller sets to ship Windows too. Off unless asked for: a
/// repository whose tools have never been built for Windows must not find
/// its next release failing on a leg it did not ask for (#232).
const OPT_IN: &str = "windows";

/// The Windows legs, label and native runner, packaged only when a caller
/// opts in.
const WINDOWS: &[(&str, &str)] = &[
    ("windows-x86_64", "windows-latest"),
    ("windows-arm64", "windows-11-arm"),
];

fn load(yaml: &str) -> Yaml<'static> {
    let mut docs = Yaml::load_from_str(yaml).expect("the workflow parses as YAML");
    assert_eq!(docs.len(), 1, "one YAML document");
    docs.remove(0)
}

fn workflow() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(WORKFLOW);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {WORKFLOW}: {e}"))
}

/// The lines of a `run:` script that are commands, not comments.
fn commands(step: &Yaml) -> Vec<String> {
    let Some(run) = step.as_mapping_get("run").and_then(Yaml::as_str) else {
        return Vec::new();
    };
    run.replace("\\\n", " ")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Whether any command in `step` starts with `program`.
fn runs(step: &Yaml, program: &str) -> bool {
    commands(step).iter().any(|c| c.starts_with(program))
}

fn uses<'a>(step: &'a Yaml<'a>) -> &'a str {
    step.as_mapping_get("uses")
        .and_then(Yaml::as_str)
        .unwrap_or("")
}

/// Every grant in `permissions` that is `write`, by name. `write-all`
/// grants every one.
fn write_grants(permissions: Option<&Yaml>) -> Vec<String> {
    let Some(permissions) = permissions else {
        return Vec::new();
    };
    if permissions.as_str() == Some("write-all") {
        return GRANTS.iter().map(|g| (*g).to_owned()).collect();
    }
    let Some(map) = permissions.as_mapping() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(_, v)| v.as_str() == Some("write"))
        .filter_map(|(k, _)| k.as_str().map(str::to_owned))
        .collect()
}

fn is_full_sha(pin: &str) -> bool {
    pin.len() == 40
        && pin
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn steps<'a>(job: &'a Yaml<'a>) -> Vec<&'a Yaml<'a>> {
    job.as_mapping_get("steps")
        .and_then(Yaml::as_sequence)
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

/// The job names in `needs`, whether it is one string or a list.
fn needs(job: &Yaml) -> Vec<String> {
    let Some(n) = job.as_mapping_get("needs") else {
        return Vec::new();
    };
    match n.as_str() {
        Some(one) => vec![one.to_owned()],
        None => n
            .as_sequence()
            .map(|s| {
                s.iter()
                    .filter_map(Yaml::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// `(label, runner)` of every entry in a sequence of matrix entries.
fn legs(entries: &Yaml) -> BTreeSet<(String, String)> {
    entries
        .as_sequence()
        .map(|s| {
            s.iter()
                .filter_map(|e| {
                    let label = e.as_mapping_get("label").and_then(Yaml::as_str)?;
                    let runner = e.as_mapping_get("runner").and_then(Yaml::as_str)?;
                    Some((label.to_owned(), runner.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

type Legs = BTreeSet<(String, String)>;

/// The legs `strategy.matrix.include` runs, `(label, runner)`, for a caller
/// that leaves the opt-in off and for one that sets it. A static list runs
/// the same legs for both. The only other shape read is the one the
/// workflow uses,
///
/// ```text
/// ${{ fromJSON(inputs.windows && '<legs when on>' || '<legs when off>') }}
/// ```
///
/// with each list in JSON, which is YAML's flow style; anything else is
/// read as no legs at all, which no caller can release with.
fn labels(job: &Yaml) -> (Legs, Legs) {
    let Some(include) = job
        .as_mapping_get("strategy")
        .and_then(|s| s.as_mapping_get("matrix"))
        .and_then(|m| m.as_mapping_get("include"))
    else {
        return Default::default();
    };
    if include.as_sequence().is_some() {
        let both = legs(include);
        return (both.clone(), both);
    }
    let expr = include.as_str().unwrap_or("").trim();
    let on_prefix = format!("${{{{ fromJSON(inputs.{OPT_IN} && '");
    let parsed = expr
        .strip_prefix(on_prefix.as_str())
        .and_then(|rest| rest.strip_suffix("') }}"))
        .and_then(|rest| rest.split_once("' || '"));
    let Some((on, off)) = parsed else {
        return Default::default();
    };
    (legs(&load(off)), legs(&load(on)))
}

/// Everything wrong with `yaml` as the family's release-tarball workflow;
/// empty when nothing is.
fn gaps(yaml: &str) -> Vec<String> {
    let doc = load(yaml);
    let mut gaps = Vec::new();

    // Callable, and only callable: a tag push here must not run it against
    // this repository, which has no command-line tool to package.
    let on = doc.as_mapping_get("on").and_then(Yaml::as_mapping);
    let triggers: Vec<&str> = on
        .map(|m| m.keys().filter_map(Yaml::as_str).collect())
        .unwrap_or_default();
    if triggers != ["workflow_call"] {
        gaps.push(format!(
            "the workflow is triggered by {triggers:?}, not by workflow_call alone"
        ));
    }
    let inputs = on
        .and_then(|m| m.get(&Yaml::value_from_str("workflow_call")))
        .and_then(|c| c.as_mapping_get("inputs"));
    for input in INPUTS {
        let required = inputs
            .and_then(|i| i.as_mapping_get(input))
            .and_then(|i| i.as_mapping_get("required"))
            .and_then(Yaml::as_bool);
        if required != Some(true) {
            gaps.push(format!("input {input} is not declared required"));
        }
    }
    let opt_in = inputs.and_then(|i| i.as_mapping_get(OPT_IN));
    let off_by_default = opt_in.is_some_and(|i| {
        i.as_mapping_get("type").and_then(Yaml::as_str) == Some("boolean")
            && i.as_mapping_get("default").and_then(Yaml::as_bool) == Some(false)
            && i.as_mapping_get("required").and_then(Yaml::as_bool) != Some(true)
    });
    if !off_by_default {
        gaps.push(format!(
            "input {OPT_IN} is not an optional boolean that defaults to false"
        ));
    }

    for grant in write_grants(doc.as_mapping_get("permissions")) {
        gaps.push(format!(
            "the workflow-level permissions grant {grant}: write to every job"
        ));
    }

    let jobs_node = doc.as_mapping_get("jobs").expect("the workflow has jobs");
    let jobs = jobs_node
        .as_mapping()
        .expect("the workflow's jobs are a mapping");
    let mut packaging = Vec::new();
    let mut attaching = Vec::new();
    for (name, job) in jobs {
        let name = name.as_str().unwrap_or("?").to_owned();
        let steps = steps(job);
        for step in &steps {
            let u = uses(step);
            if !u.is_empty() && !u.starts_with("./") {
                let pin = u.split_once('@').map(|(_, p)| p).unwrap_or("");
                if !is_full_sha(pin) {
                    gaps.push(format!(
                        "job {name} uses {u}, which a moved tag can redirect; pin a full commit SHA"
                    ));
                }
            }
            // A caller's input is text: spliced into a script it is code.
            if let Some(run) = step.as_mapping_get("run").and_then(Yaml::as_str) {
                if run.contains("inputs.") && run.contains("${{") {
                    gaps.push(format!(
                        "job {name} expands an input inside a run script; pass it through env"
                    ));
                }
            }
            if commands(step)
                .iter()
                .any(|c| c.contains("scripts/package-cli.sh") && !c.contains(PACKAGE))
            {
                gaps.push(format!(
                    "job {name} runs a local scripts/package-cli.sh, not {PACKAGE}"
                ));
            }
            // The caller keeps no scripts/core.sh: the family runs core's
            // scripts in place (#212), so a step that reaches for the shim
            // fails on every caller with "No such file or directory".
            if commands(step).iter().any(|c| c.contains("scripts/core.sh")) {
                gaps.push(format!(
                    "job {name} runs scripts/core.sh, which no caller carries; run {PACKAGE}"
                ));
            }
        }
        if steps.iter().any(|s| runs(s, PACKAGE)) {
            packaging.push(name.clone());
        }
        if steps.iter().any(|s| uses(s).starts_with(ATTEST)) {
            attaching.push(name);
        }
    }

    if packaging.len() != 1 {
        gaps.push(format!(
            "{} jobs run `{PACKAGE}`, not one: {packaging:?}",
            packaging.len()
        ));
    }
    if attaching.len() != 1 {
        gaps.push(format!(
            "{} jobs use {ATTEST}<sha>, not one: {attaching:?}",
            attaching.len()
        ));
    }

    for (name, job) in jobs {
        let name = name.as_str().unwrap_or("?");
        let granted = write_grants(job.as_mapping_get("permissions"));
        if !attaching.iter().any(|a| a == name) {
            for grant in granted {
                gaps.push(format!(
                    "job {name} attests nothing but holds {grant}: write"
                ));
            }
        }
    }

    let (Some(pack), Some(attach)) = (packaging.first(), attaching.first()) else {
        return gaps;
    };
    let pack_job = jobs_node.as_mapping_get(pack).expect("named job");
    let attach_job = jobs_node.as_mapping_get(attach).expect("named job");
    if pack == attach {
        gaps.push(format!(
            "job {pack} both builds and attests, so the build holds the release's grants"
        ));
        return gaps;
    }

    let want: BTreeSet<String> = LABELS.iter().map(|l| (*l).to_owned()).collect();
    let (off, on) = labels(pack_job);
    let have: BTreeSet<String> = off.iter().map(|(l, _)| l.clone()).collect();
    if have != want {
        gaps.push(format!(
            "job {pack} packages for {have:?}, not every platform the family ships {want:?}"
        ));
    }
    // With the opt-in set: the same legs, plus each Windows one on its own
    // native runner, and nothing else.
    let mut want_on: BTreeSet<(String, String)> = off
        .iter()
        .filter(|(l, _)| want.contains(l))
        .cloned()
        .collect();
    want_on.extend(
        WINDOWS
            .iter()
            .map(|(l, r)| ((*l).to_owned(), (*r).to_owned())),
    );
    if on != want_on {
        gaps.push(format!(
            "job {pack} packages for {on:?} when {OPT_IN} is set, not {want_on:?}"
        ));
    }
    let pack_steps = steps(pack_job);
    let packs_at = pack_steps
        .iter()
        .position(|s| runs(s, PACKAGE))
        .expect("found above");
    if !pack_steps[..packs_at]
        .iter()
        .any(|s| runs(s, "cargo build --release --locked"))
    {
        gaps.push(format!(
            "job {pack} packages before any `cargo build --release --locked`"
        ));
    }
    if !pack_steps[packs_at + 1..]
        .iter()
        .any(|s| uses(s).starts_with("actions/upload-artifact@"))
    {
        gaps.push(format!(
            "job {pack} does not hand its tarball on as an artifact"
        ));
    }

    if !needs(attach_job).iter().any(|n| n == pack) {
        gaps.push(format!(
            "job {attach} does not need {pack}, so it can attach before every leg passed"
        ));
    }
    let attach_steps = steps(attach_job);
    let at = attach_steps
        .iter()
        .position(|s| uses(s).starts_with(ATTEST))
        .expect("found above");
    if !attach_steps[..at]
        .iter()
        .any(|s| uses(s).starts_with("actions/download-artifact@"))
    {
        gaps.push(format!(
            "job {attach} attests before collecting the legs' tarballs"
        ));
    }
    // One asset per leg that ran: the count follows the opt-in, so a
    // release can never carry the Windows assets without the others, nor
    // the others with a Windows leg missing.
    let count = format!(
        "${{{{ inputs.{OPT_IN} && {} || {} }}}}",
        LABELS.len() + WINDOWS.len(),
        LABELS.len()
    );
    if !attach_steps[..at].iter().any(|s| {
        s.as_mapping_get("env")
            .and_then(|e| e.as_mapping_get("EXPECTED"))
            .and_then(Yaml::as_str)
            == Some(count.as_str())
            && commands(s).iter().any(|c| c.contains("-ne \"$EXPECTED\""))
    }) {
        gaps.push(format!(
            "job {attach} does not refuse a set of assets other than one per leg (EXPECTED: {count})"
        ));
    }
    // dist/ holds the legs' assets and nothing else, a .tar.gz or a .zip
    // each: attesting and uploading all of it attests and uploads them all.
    let subject = attach_steps[at]
        .as_mapping_get("with")
        .and_then(|w| w.as_mapping_get("subject-path"))
        .and_then(Yaml::as_str)
        .unwrap_or("");
    if subject != "dist/*" {
        gaps.push(format!(
            "job {attach} attests {subject:?}, not every asset in dist/*"
        ));
    }
    if !attach_steps[at + 1..].iter().any(|s| {
        commands(s)
            .iter()
            .any(|c| c.starts_with("gh release upload") && c.contains(" dist/* "))
    }) {
        gaps.push(format!(
            "job {attach} does not attach every attested asset in dist/* to the GitHub release"
        ));
    }
    let granted = write_grants(attach_job.as_mapping_get("permissions"));
    for grant in GRANTS {
        if !granted.iter().any(|g| g == grant) {
            gaps.push(format!("job {attach} attests without {grant}: write"));
        }
    }
    gaps
}

#[test]
fn the_release_cli_workflow_packages_attests_and_attaches_from_one_place() {
    let gaps = gaps(&workflow());
    assert!(
        gaps.is_empty(),
        "{WORKFLOW} must be callable only, package every platform through \
         core's package-cli.sh in place, and attest and attach from the one privileged job: {gaps:#?}"
    );
}

/// The reader answers for the inputs it is meant to catch, and not for
/// the ones it is not.
#[test]
fn the_reader_discriminates() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    // The legs, as the workflow writes them: JSON, one list for a caller
    // that leaves the opt-in off and one for a caller that sets it.
    let unix = r#"{"runner":"macos-latest","label":"darwin-arm64"},{"runner":"ubuntu-latest","label":"linux-x86_64"}"#;
    let win = r#"{"runner":"windows-latest","label":"windows-x86_64"},{"runner":"windows-11-arm","label":"windows-arm64"}"#;
    let off = format!("[{unix}]");
    let on = format!("[{unix},{win}]");
    let good = r#"on:
  workflow_call:
    inputs:
      core-ref:
        required: true
        type: string
      toolchain:
        required: true
        type: string
      windows:
        required: false
        type: boolean
        default: false
permissions: {}
jobs:
  package:
    permissions:
      contents: read
    strategy:
      matrix:
        include: ${{ fromJSON(inputs.windows && '@ON@' || '@OFF@') }}
    steps:
      - uses: actions/checkout@SHA # v5
      - env:
          T: ${{ inputs.toolchain }}
        run: echo "$T"
      - run: cargo build --release --locked --features cli --bin x
      - run: bash ../rust-fs-core/scripts/package-cli.sh 1 l
      - uses: actions/upload-artifact@SHA # v4
  attach:
    needs: package
    permissions:
      id-token: write
      attestations: write
      contents: write
    steps:
      - uses: actions/download-artifact@SHA # v4
      - env:
          EXPECTED: ${{ inputs.windows && 4 || 2 }}
        run: |
          if [ "${#a[@]}" -ne "$EXPECTED" ]; then exit 1; fi
      - uses: ATTESTSHA # v4.2.2
        with:
          subject-path: dist/*
      - run: gh release upload "$GITHUB_REF_NAME" dist/* --clobber
"#
    .replace("@ON@", &on)
    .replace("@OFF@", &off)
    .replace("ATTEST", ATTEST)
    .replace("SHA", sha);
    assert_eq!(gaps(&good), Vec::<String>::new(), "{good}");

    let expect = |yaml: String, want: &str| {
        let gaps = gaps(&yaml);
        assert!(
            gaps.iter().any(|g| g.contains(want)),
            "expected a gap mentioning {want:?}, got {gaps:#?} for\n{yaml}"
        );
    };
    // Triggered by a tag push too.
    expect(
        good.replace("on:\n", "on:\n  push:\n    tags: ['v*']\n"),
        "not by workflow_call alone",
    );
    // An input not required.
    expect(
        good.replace(
            "toolchain:\n        required: true",
            "toolchain:\n        required: false",
        ),
        "input toolchain is not declared required",
    );
    // Pinned to a tag.
    expect(
        good.replacen(&format!("upload-artifact@{sha}"), "upload-artifact@v4", 1),
        "pin a full commit SHA",
    );
    // An input spliced into a script.
    expect(
        good.replace(
            "run: cargo build --release --locked --features cli --bin x",
            "run: cargo build --release --locked --features cli --bin ${{ inputs.toolchain }}",
        ),
        "expands an input inside a run script",
    );
    // A local copy of the script.
    expect(
        good.replace(
            "bash ../rust-fs-core/scripts/package-cli.sh 1 l",
            "scripts/package-cli.sh 1 l",
        ),
        "jobs run `bash ../rust-fs-core/scripts/package-cli.sh`, not one",
    );
    // The shim no caller carries.
    expect(
        good.replace(
            "bash ../rust-fs-core/scripts/package-cli.sh 1 l",
            "bash scripts/core.sh package-cli 1 l",
        ),
        "runs scripts/core.sh, which no caller carries",
    );
    expect(
        good.replace(
            "      - uses: actions/upload-artifact",
            "      - run: scripts/package-cli.sh 1 l\n      - uses: actions/upload-artifact",
        ),
        "runs a local scripts/package-cli.sh",
    );
    // A platform dropped.
    expect(
        good.replace(r#"{"runner":"macos-latest","label":"darwin-arm64"},"#, ""),
        "not every platform the family ships",
    );
    // Packaged before building, or never handed on.
    expect(
        good.replace(
            "      - run: cargo build --release --locked --features cli --bin x\n",
            "",
        ),
        "packages before any `cargo build --release --locked`",
    );
    expect(
        good.replace(
            &format!("      - uses: actions/upload-artifact@{sha} # v4\n"),
            "",
        ),
        "does not hand its tarball on",
    );
    // Attaching without waiting for the legs.
    expect(
        good.replace("    needs: package\n", ""),
        "does not need package",
    );
    // No count of the tarballs, or the wrong one.
    expect(
        good.replace("&& 4 || 2", "&& 2 || 2"),
        "one per leg (EXPECTED: ${{ inputs.windows && 4 || 2 }})",
    );
    expect(good.replace("-ne \"$EXPECTED\"", "-ne 2"), "one per leg");
    // The opt-in missing, required, or on by default.
    expect(
        good.replace("      windows:\n        required: false\n        type: boolean\n        default: false\n", ""),
        "input windows is not an optional boolean that defaults to false",
    );
    expect(
        good.replace("default: false", "default: true"),
        "input windows is not an optional boolean that defaults to false",
    );
    expect(
        good.replace("required: false", "required: true"),
        "input windows is not an optional boolean that defaults to false",
    );
    // A Windows leg dropped, on the wrong runner, or run for a caller that
    // did not ask.
    expect(
        good.replace(
            r#",{"runner":"windows-11-arm","label":"windows-arm64"}"#,
            "",
        ),
        "when windows is set",
    );
    expect(
        good.replace(
            r#""runner":"windows-11-arm""#,
            r#""runner":"windows-latest""#,
        ),
        "when windows is set",
    );
    expect(
        good.replace(&format!("|| '{off}'"), &format!("|| '{on}'")),
        "not every platform the family ships",
    );
    // Each grant dropped in turn.
    for grant in GRANTS {
        expect(
            good.replace(&format!("      {grant}: write\n"), ""),
            &format!("attests without {grant}: write"),
        );
    }
    // A grant hoisted to the whole workflow, or given to the build.
    expect(
        good.replace("permissions: {}\n", "permissions: write-all\n"),
        "workflow-level permissions grant attestations",
    );
    expect(
        good.replace("      contents: read\n", "      contents: write\n"),
        "job package attests nothing but holds contents: write",
    );
    // Attesting the wrong thing, or not attaching it.
    expect(
        good.replace("subject-path: dist/*", "subject-path: dist/*.tar.gz"),
        "not every asset in dist/*",
    );
    expect(
        good.replace("gh release upload", "echo gh-release-upload"),
        "does not attach every attested asset",
    );
    // The attest step gone, or only named in a comment.
    expect(
        good.replace(
            &format!("      - uses: {ATTEST}{sha} # v4.2.2\n        with:\n          subject-path: dist/*\n"),
            "      # uses: actions/attest-build-provenance\n",
        ),
        "jobs use actions/attest-build-provenance@<sha>, not one",
    );
}
