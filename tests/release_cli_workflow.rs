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

/// The grants the attaching job needs, each at `write`: an OIDC token to
/// sign with, the attestation store, and the release to attach to.
const GRANTS: &[&str] = &["id-token", "attestations", "contents"];

/// The inputs a caller must pass: the rust-fs-core tag its `Cargo.toml`
/// pins (the path sibling the build needs), and its pinned toolchain.
const INPUTS: &[&str] = &["core-ref", "toolchain"];

/// The platforms every repository ships, one native runner each.
const LABELS: &[&str] = &["darwin-arm64", "linux-x86_64"];

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

/// The `label` of every `strategy.matrix.include` entry.
fn labels(job: &Yaml) -> BTreeSet<String> {
    job.as_mapping_get("strategy")
        .and_then(|s| s.as_mapping_get("matrix"))
        .and_then(|m| m.as_mapping_get("include"))
        .and_then(Yaml::as_sequence)
        .map(|s| {
            s.iter()
                .filter_map(|e| e.as_mapping_get("label").and_then(Yaml::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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
                .any(|c| c.contains("scripts/package-cli.sh"))
            {
                gaps.push(format!(
                    "job {name} runs a local scripts/package-cli.sh, not core.sh package-cli"
                ));
            }
        }
        if steps
            .iter()
            .any(|s| runs(s, "bash scripts/core.sh package-cli"))
        {
            packaging.push(name.clone());
        }
        if steps.iter().any(|s| uses(s).starts_with(ATTEST)) {
            attaching.push(name);
        }
    }

    if packaging.len() != 1 {
        gaps.push(format!(
            "{} jobs run `bash scripts/core.sh package-cli`, not one: {packaging:?}",
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
    let have = labels(pack_job);
    if have != want {
        gaps.push(format!(
            "job {pack} packages for {have:?}, not every platform the family ships {want:?}"
        ));
    }
    let pack_steps = steps(pack_job);
    let packs_at = pack_steps
        .iter()
        .position(|s| runs(s, "bash scripts/core.sh package-cli"))
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
    let count = format!("-ne {}", LABELS.len());
    if !attach_steps[..at]
        .iter()
        .any(|s| commands(s).iter().any(|c| c.contains(&count)))
    {
        gaps.push(format!(
            "job {attach} does not refuse a set of tarballs other than one per platform ({count})"
        ));
    }
    let subject = attach_steps[at]
        .as_mapping_get("with")
        .and_then(|w| w.as_mapping_get("subject-path"))
        .and_then(Yaml::as_str)
        .unwrap_or("");
    if !subject.ends_with(".tar.gz") {
        gaps.push(format!(
            "job {attach} attests {subject:?}, not the tarballs"
        ));
    }
    if !attach_steps[at + 1..].iter().any(|s| {
        commands(s)
            .iter()
            .any(|c| c.starts_with("gh release upload") && c.contains(".tar.gz"))
    }) {
        gaps.push(format!(
            "job {attach} does not attach the attested tarballs to the GitHub release"
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
         core.sh, and attest and attach from the one privileged job: {gaps:#?}"
    );
}

/// The reader answers for the inputs it is meant to catch, and not for
/// the ones it is not.
#[test]
fn the_reader_discriminates() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let good = format!(
        "on:\n  workflow_call:\n    inputs:\n      core-ref:\n        required: true\n        type: string\n\
         \x20     toolchain:\n        required: true\n        type: string\n\
         permissions: {{}}\n\
         jobs:\n  package:\n    permissions:\n      contents: read\n\
         \x20   strategy:\n      matrix:\n        include:\n          - label: darwin-arm64\n          - label: linux-x86_64\n\
         \x20   steps:\n      - uses: actions/checkout@{sha} # v5\n\
         \x20     - env:\n          T: ${{{{ inputs.toolchain }}}}\n        run: echo \"$T\"\n\
         \x20     - run: cargo build --release --locked --features cli --bin x\n\
         \x20     - run: bash scripts/core.sh package-cli 1 l\n\
         \x20     - uses: actions/upload-artifact@{sha} # v4\n\
         \x20 attach:\n    needs: package\n    permissions:\n      id-token: write\n      attestations: write\n      contents: write\n\
         \x20   steps:\n      - uses: actions/download-artifact@{sha} # v4\n\
         \x20     - run: |\n          if [ \"${{#a[@]}}\" -ne 2 ]; then exit 1; fi\n\
         \x20     - uses: {ATTEST}{sha} # v4.2.2\n        with:\n          subject-path: dist/*.tar.gz\n\
         \x20     - run: gh release upload \"$GITHUB_REF_NAME\" dist/*.tar.gz --clobber\n"
    );
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
            "bash scripts/core.sh package-cli 1 l",
            "scripts/package-cli.sh 1 l",
        ),
        "jobs run `bash scripts/core.sh package-cli`, not one",
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
        good.replace("          - label: darwin-arm64\n", ""),
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
    expect(good.replace("-ne 2", "-ne 1"), "one per platform (-ne 2)");
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
        good.replace("subject-path: dist/*.tar.gz", "subject-path: Cargo.toml"),
        "not the tarballs",
    );
    expect(
        good.replace("gh release upload", "echo gh-release-upload"),
        "does not attach the attested tarballs",
    );
    // The attest step gone, or only named in a comment.
    expect(
        good.replace(
            &format!("      - uses: {ATTEST}{sha} # v4.2.2\n        with:\n          subject-path: dist/*.tar.gz\n"),
            "      # uses: actions/attest-build-provenance\n",
        ),
        "jobs use actions/attest-build-provenance@<sha>, not one",
    );
}
