//! Every CI step that runs the suite acknowledges the privilege gap.
//!
//! `tests/device_node_size.rs` FAILS rather than skips when it cannot
//! build a loop device, unless `AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP` is
//! set -- because libtest discards a passing test's output, so a
//! printed "skipped" line reaches nobody. That design puts the
//! acknowledgement in `ci.yml`, where a person reads it in review.
//!
//! It also means a step that runs the suite WITHOUT the variable fails,
//! and that is not hypothetical: the variable was set on the `test`
//! job's step only, the `coverage` job runs the suite again under
//! llvm-cov, and it failed there while `test` passed on the same
//! commit. Nothing connected the two, because the acknowledgement was
//! attached to one step rather than to the property "this step runs the
//! suite".
//!
//! # This parses the workflow rather than scanning it
//!
//! The first version of this file scanned lines, and asked whether the
//! variable appeared anywhere in the JOB -- a bound it stated but could
//! not escape, because finding which step an `env:` belongs to is
//! structure, and a line scan has none. Three ordinary things defeat or
//! mislead such a scan:
//!
//! - a quoted key, `"env":`, which is the same key;
//! - a flow mapping, `env: { NAME: "1" }`, which puts a mapping on one
//!   line;
//! - a comment naming the variable, which this file's `test` job has
//!   three lines above where it sets it, so a comment-blind scan passes
//!   with the `env:` deleted.
//!
//! Parsed, none of those is an edge case: they are the grammar, and the
//! quoting and the YAML comments are resolved before this file sees
//! anything. (A `#` inside a `run: |` block is shell, not YAML, and
//! reaches `runs_the_suite` as text; it strips those itself.) And the question becomes the accurate one -- does the STEP
//! that runs the suite have the variable, on itself, on its job, or on
//! the workflow -- which is the defeat the scan actually allowed.
//!
//! ONE FEAR THAT TURNED OUT TO BE UNFOUNDED, recorded so it is not
//! re-raised: a `run: |` block whose contents contain a line that looks
//! like a job key cannot mislead an indentation scanner, because such a
//! line would have to sit at the job indent, and a line less indented
//! than the block ENDS the block. It is not expressible in valid YAML.
//! The first draft of this file asserted it and the fixture would not
//! parse.

use saphyr::{LoadableYamlNode, Yaml};
use std::path::PathBuf;

const VAR: &str = "AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP";

fn ci_yml() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join("ci.yml")
}

/// The value of `name` in a YAML mapping, or `None`.
fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// Whether an `env:` mapping declares the variable.
///
/// The parser has resolved the quoting, so `NAME`, `"NAME"` and
/// `'NAME'` all arrive here the same. That is the whole of the
/// quoted-key question: there is no un-quoting step to forget.
fn env_declares(env: Option<&Yaml>, name: &str) -> bool {
    env.and_then(|e| e.as_mapping())
        .map(|m| m.iter().any(|(k, _)| k.as_str() == Some(name)))
        .unwrap_or(false)
}

/// Whether a `run:` script runs the test suite.
///
/// `cargo test`, its `cargo t` alias, `cargo llvm-cov` and `cargo nextest`
/// all run it. So does a script the step calls that runs one of those:
/// a script path the step names that exists in this repository is read
/// and asked the same question. This was a two-spelling substring match,
/// so a suite run through `./scripts/ci-test.sh` or `cargo nextest` was
/// exempt, and the controls below read the extra, unguarded jobs as a
/// healthier ratio (#125).
///
/// Shell comments are removed first: a `run: |` block is shell, and
/// `# see also cargo test` in it runs nothing. (YAML comments never reach
/// here; the parser drops them.)
fn runs_the_suite(run: &str) -> bool {
    runs_the_suite_within(run, 3)
}

fn runs_the_suite_within(script: &str, depth: u8) -> bool {
    let code: String = script
        .lines()
        .map(strip_shell_comment)
        .collect::<Vec<_>>()
        .join("\n");
    let words: Vec<&str> = code
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`'))
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .collect();
    let direct = words
        .windows(2)
        .any(|pair| pair[0] == "cargo" && matches!(pair[1], "test" | "t" | "llvm-cov" | "nextest"));
    direct
        || (depth > 0
            && words.iter().any(|word| {
                repository_script(word)
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .is_some_and(|body| runs_the_suite_within(&body, depth - 1))
            }))
}

/// `line` up to a `#` that begins a word, which is where a shell comment
/// starts. Quotes are not tracked: a `#` inside a quoted string that
/// follows a space is cut too, which can only make a step look like it
/// runs less, and a quoted `cargo test` is not a run anyway.
fn strip_shell_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// `word` as a file in this repository, if it names one: `scripts/x.sh`,
/// `./scripts/x.sh`, or an absolute path.
fn repository_script(word: &str) -> Option<PathBuf> {
    if !(word.contains('/') || word.ends_with(".sh")) {
        return None;
    }
    let path = PathBuf::from(word);
    let path = if path.is_absolute() {
        path
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(word.trim_start_matches("./"))
    };
    path.is_file().then_some(path)
}

fn parse(text: &str) -> Yaml<'_> {
    let documents = Yaml::load_from_str(text).unwrap_or_else(|e| {
        panic!(
            "ci.yml is not valid YAML: {e}. This guard reads the workflow rather \
             than scanning its text, so a file it cannot parse is a failure and \
             never a pass."
        )
    });
    documents
        .into_iter()
        .next()
        .expect("ci.yml must contain a document")
}

/// Every step that runs the suite without the acknowledgement, named
/// `job / step`.
fn unacknowledged(text: &str) -> Vec<String> {
    let doc = parse(text);
    let workflow_env = field(&doc, "env").cloned();
    let Some(jobs) = field(&doc, "jobs").and_then(|j| j.as_mapping().cloned()) else {
        panic!("ci.yml has no `jobs:` mapping; this guard would assert nothing");
    };

    let mut out = Vec::new();
    for (job_name, job) in jobs.iter() {
        let job_name = job_name.as_str().unwrap_or("<unnamed>");
        let job_env = field(job, "env").cloned();
        let Some(steps) = field(job, "steps").and_then(|s| s.as_sequence()) else {
            continue;
        };
        for (i, step) in steps.iter().enumerate() {
            let run = field(step, "run").and_then(|r| r.as_str()).unwrap_or("");
            if !runs_the_suite(run) {
                continue;
            }
            let step_env = field(step, "env").cloned();
            let acknowledged = env_declares(step_env.as_ref(), VAR)
                || env_declares(job_env.as_ref(), VAR)
                || env_declares(workflow_env.as_ref(), VAR);
            if !acknowledged {
                let label = field(step, "name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("step {i}"));
                out.push(format!("{job_name} / {label}"));
            }
        }
    }
    out
}

/// Jobs with at least one step that runs the suite.
fn jobs_running_the_suite(text: &str) -> Vec<String> {
    let doc = parse(text);
    let Some(jobs) = field(&doc, "jobs").and_then(|j| j.as_mapping().cloned()) else {
        return Vec::new();
    };
    jobs.iter()
        .filter(|(_, job)| {
            field(job, "steps")
                .and_then(|s| s.as_sequence())
                .map(|steps| {
                    steps.iter().any(|step| {
                        runs_the_suite(field(step, "run").and_then(|r| r.as_str()).unwrap_or(""))
                    })
                })
                .unwrap_or(false)
        })
        .map(|(name, _)| name.as_str().unwrap_or("<unnamed>").to_string())
        .collect()
}

#[test]
fn every_step_that_runs_the_suite_acknowledges_the_privilege_gap() {
    let text = std::fs::read_to_string(ci_yml()).expect("read .github/workflows/ci.yml");
    let running = jobs_running_the_suite(&text);
    assert!(
        running.len() >= 2,
        "control: expected at least the `test` and `coverage` jobs to run the suite, \
         found {running:?}. If this drops to one, the check below measures less than \
         it looks."
    );
    let silent = unacknowledged(&text);
    assert!(
        silent.is_empty(),
        "these ci.yml steps run the test suite without {VAR}: {silent:?}. \
         tests/device_node_size.rs fails rather than skipping when it cannot build a \
         loop device, so such a step fails on a runner that is not root -- which is \
         exactly how `coverage` broke while `test` passed on the same commit."
    );
}

/// The classifier has to discriminate. If everything looked like a
/// suite run, the check above would be asserting the variable is set in
/// every job, and `fmt` -- which has no business setting it -- would
/// say so.
#[test]
fn a_job_that_does_not_run_the_suite_is_not_asked_to_acknowledge() {
    let text = std::fs::read_to_string(ci_yml()).expect("read ci.yml");
    let doc = parse(&text);
    let all = field(&doc, "jobs")
        .and_then(|j| j.as_mapping().cloned())
        .expect("jobs")
        .len();
    let running = jobs_running_the_suite(&text).len();
    assert!(
        running < all,
        "control: all {all} jobs look like they run the suite, so the classifier is \
         not discriminating and the check above proves nothing about which need this"
    );
}

mod parser {
    use super::{unacknowledged, VAR};

    /// A workflow with the acknowledgement in the ordinary place.
    const OK: &str = "\
on:
  pull_request:
jobs:
  test:
    steps:
      - run: cargo test --locked
        env:
          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"
";

    #[test]
    fn the_control_shape_is_acknowledged() {
        assert!(
            unacknowledged(OK).is_empty(),
            "the control must pass, or every test below passes for the wrong reason"
        );
    }

    #[test]
    fn a_step_with_no_acknowledgement_anywhere_is_reported() {
        let yaml = OK.replace(
            "        env:\n          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"\n",
            "",
        );
        assert_eq!(
            unacknowledged(&yaml).len(),
            1,
            "{VAR} is nowhere in the job"
        );
    }

    /// LEGAL INPUT A LINE SCAN GOT WRONG. A quoted key is the same key,
    /// and the parser has already resolved it -- there is no unquoting
    /// step to forget.
    #[test]
    fn a_quoted_env_key_is_the_same_key() {
        let yaml = OK.replace(
            "          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"",
            "          \"AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP\": \"1\"",
        );
        assert!(unacknowledged(&yaml).is_empty(), "a quoted key is the key");
    }

    /// LEGAL INPUT A LINE SCAN GOT WRONG. A flow mapping puts the whole
    /// `env:` on one line.
    #[test]
    fn a_flow_mapping_env_is_read() {
        let yaml = OK.replace(
            "        env:\n          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"\n",
            "        env: { AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\" }\n",
        );
        assert!(
            unacknowledged(&yaml).is_empty(),
            "`env: {{ NAME: \"1\" }}` declares the same variable"
        );
    }

    /// LEGAL INPUT A LINE SCAN GOT WRONG. A comment is not data, and
    /// this file's own `test` job names the variable in a comment three
    /// lines above where it sets it -- so a comment-blind check passes
    /// with the `env:` deleted and the paragraph left behind. The
    /// previous version handled this with a hand-rolled stripping pass;
    /// the parser never sees a comment at all.
    #[test]
    fn a_comment_naming_the_variable_does_not_acknowledge() {
        let yaml = OK.replace(
            "        env:\n          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"\n",
            "        # AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP acknowledges a real gap\n",
        );
        assert_eq!(
            unacknowledged(&yaml).len(),
            1,
            "the variable is named in prose and set nowhere"
        );
    }

    /// THE SPELLINGS A SUBSTRING MISSED (#125): a script that runs the
    /// suite, and `cargo nextest`. Each is reported when nothing
    /// acknowledges the gap.
    #[test]
    fn a_suite_run_under_another_spelling_is_reported() {
        let dir = std::env::temp_dir().join(format!("core-ci-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("ci-test.sh");
        std::fs::write(&script, "#!/bin/sh\nset -e\ncargo test --locked \"$@\"\n").unwrap();
        let quiet = dir.join("lint.sh");
        std::fs::write(
            &quiet,
            "#!/bin/sh\n# cargo test runs elsewhere\ncargo fmt --check\n",
        )
        .unwrap();

        for run in [
            format!("{} --test device_node_size", script.display()),
            format!("bash {}", script.display()),
            "cargo nextest run --locked".to_string(),
            "cargo t --locked".to_string(),
        ] {
            let yaml =
                format!("on:\n  pull_request:\njobs:\n  test:\n    steps:\n      - run: {run}\n");
            assert_eq!(unacknowledged(&yaml).len(), 1, "{run:?} runs the suite");
        }
        // A quoted `cargo test` still counts -- `bash -c "cargo test"` is a
        // run -- so the only refusal here is a script that does not run it.
        // Over-asking for the acknowledgement is the harmless direction.
        let run = quiet.display();
        let yaml =
            format!("on:\n  pull_request:\njobs:\n  test:\n    steps:\n      - run: {run}\n");
        assert!(
            unacknowledged(&yaml).is_empty(),
            "{run} does not run the suite"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shell comment inside a `run:` block runs nothing (#125).
    #[test]
    fn a_shell_comment_in_a_run_block_is_not_a_run() {
        let yaml = "\
on:
  pull_request:
jobs:
  lint:
    steps:
      - run: |
          # see also cargo test --locked, in the test job
          cargo fmt --check
";
        assert!(
            unacknowledged(yaml).is_empty(),
            "a comment names the suite and runs nothing"
        );
    }

    /// A `run:` written as a block scalar is still a run. All three
    /// spellings, because `|` is not the only one and a sibling guard
    /// in this family was defeated by exactly that.
    #[test]
    fn a_block_scalar_run_is_read_in_any_spelling() {
        for opener in ["|", "|-", ">"] {
            let yaml = format!(
                "\
on:
  pull_request:
jobs:
  test:
    steps:
      - run: {opener}
          set -euo pipefail
          cargo test --locked
"
            );
            assert_eq!(
                unacknowledged(&yaml).len(),
                1,
                "`run: {opener}` opens a block whose contents run the suite, and \
                 nothing acknowledges the gap"
            );
        }
    }

    /// THE DEFEAT THE LINE SCAN ALLOWED, and the reason the question is
    /// asked per STEP. The old version asked whether the variable
    /// appeared anywhere in the job, so a step that set it satisfied a
    /// different step that did not.
    #[test]
    fn a_variable_on_another_step_does_not_acknowledge_this_one() {
        let yaml = "\
on:
  pull_request:
jobs:
  test:
    steps:
      - run: echo hello
        env:
          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"
      - run: cargo test --locked
";
        assert_eq!(
            unacknowledged(yaml).len(),
            1,
            "the step that runs the suite has no acknowledgement; another step's does \
             not travel to it"
        );
    }

    /// Job-level and workflow-level `env:` DO travel to it, because
    /// GitHub says they do. Refusing them would be over-strict in the
    /// direction that breaks a correct workflow.
    #[test]
    fn a_job_or_workflow_level_env_acknowledges_every_step_in_scope() {
        let job_level = "\
on:
  pull_request:
jobs:
  test:
    env:
      AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"
    steps:
      - run: cargo test --locked
";
        assert!(
            unacknowledged(job_level).is_empty(),
            "job-level env applies"
        );

        let workflow_level = "\
on:
  pull_request:
env:
  AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"
jobs:
  test:
    steps:
      - run: cargo test --locked
";
        assert!(
            unacknowledged(workflow_level).is_empty(),
            "workflow-level env applies"
        );
    }

    /// `cargo llvm-cov` runs the suite too. This is the half that was
    /// missed and caused the failure.
    #[test]
    fn a_coverage_run_counts_as_running_the_suite() {
        let yaml = OK
            .replace(
                "cargo test --locked",
                "cargo llvm-cov --fail-under-lines 90",
            )
            .replace(
                "        env:\n          AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP: \"1\"\n",
                "",
            );
        assert_eq!(unacknowledged(&yaml).len(), 1, "llvm-cov runs the tests");
    }
}
