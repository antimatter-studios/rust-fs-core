//! Behavior contract for the canonical `scripts/output-budget.sh`.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bash() -> PathBuf {
    if cfg!(windows) {
        let git_bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        if git_bash.is_file() {
            return git_bash;
        }
    }
    PathBuf::from("bash")
}

fn log_path(name: &str) -> String {
    let directory = repo().join("tmp").join("output-budget-test");
    std::fs::create_dir_all(&directory).expect("create output-budget scratch directory");
    let _ = std::fs::remove_file(directory.join(name));
    format!("tmp/output-budget-test/{name}")
}

fn run(arguments: &[&str], verbose: bool) -> Output {
    run_with_environment(arguments, verbose, &[])
}

fn run_with_environment(arguments: &[&str], verbose: bool, environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(bash());
    command
        .current_dir(repo())
        .arg("scripts/output-budget.sh")
        .args(arguments);
    if verbose {
        command.env("OUTPUT_BUDGET_VERBOSE", "1");
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap_or_else(|error| {
        panic!(
            "could not run the output-budget wrapper through {}: {error}",
            bash().display()
        )
    })
}

fn printed(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr)
}

#[test]
fn the_canonical_script_exposes_a_stable_api_version() {
    let output = run(&["--version"], false);
    assert!(output.status.success(), "{}", printed(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "rust-fs-core-output-budget 1"
    );
}

#[test]
fn a_pass_is_quiet_but_kept_and_names_its_log() {
    let log = log_path("quiet.log");
    let output = run(
        &[
            "--log",
            &log,
            "--max-lines",
            "5",
            "--label",
            "quiet",
            "--",
            "echo",
            "hello",
        ],
        false,
    );
    let terminal = printed(&output);

    assert!(output.status.success(), "{terminal}");
    assert!(
        !terminal.contains("hello"),
        "passing output leaked: {terminal}"
    );
    assert!(terminal.contains("quiet: ok") && terminal.contains(&log));
    assert_eq!(
        std::fs::read_to_string(repo().join(log)).unwrap().trim(),
        "hello"
    );
}

#[test]
fn a_loud_pass_has_the_distinct_budget_status() {
    let log = log_path("loud.log");
    let output = run(
        &[
            "--log",
            &log,
            "--max-lines",
            "2",
            "--label",
            "loud",
            "--",
            "sh",
            "-c",
            "printf 'one\\ntwo\\nthree\\n'",
        ],
        false,
    );

    assert_eq!(output.status.code(), Some(65), "{}", printed(&output));
    assert!(printed(&output).contains("3 lines (budget 2)"));
}

#[test]
fn a_byte_budget_is_enforced() {
    let log = log_path("bytes.log");
    let output = run(
        &["--log", &log, "--max-bytes", "4", "--", "echo", "12345678"],
        false,
    );
    assert_eq!(output.status.code(), Some(65), "{}", printed(&output));
}

/// AN OVER-BUDGET VERDICT SAYS WHERE THE RULE IS.
///
/// The two older copies of this script -- `fs-linux-test-harness`'s and
/// the fork `rust-img-qcow2` carried -- ended this verdict with "see the
/// output section of the consumer test contract". Core's copy dropped
/// the pointer when it became the canonical one, and with it the only
/// part of the message that answers the question the message provokes:
/// a run has just been failed for printing too much, and "raise the
/// measured budget deliberately" does not say what a deliberate raise
/// has to carry. Exit 65 is a status a reader meets having read nothing,
/// so the verdict is the whole of what it gets. rust-fs-core#153.
#[test]
fn a_loud_pass_names_the_contract_it_breached() {
    let log = log_path("loud-pointer.log");
    let output = run(
        &[
            "--log",
            &log,
            "--max-lines",
            "1",
            "--label",
            "loud-pointer",
            "--",
            "sh",
            "-c",
            "printf 'one\\ntwo\\n'",
        ],
        false,
    );
    let terminal = printed(&output);

    assert_eq!(output.status.code(), Some(65), "{terminal}");
    assert!(
        terminal.contains("docs/output-budget.md"),
        "the over-budget verdict did not say where the contract is: {terminal}"
    );
}

/// A VERDICT THAT CANNOT BE PRINTED IS NOT A FAILING TIER.
///
/// This script's exit status is a claim about the command it wrapped.
/// The final `printf` is the last statement, so without an explicit
/// `exit 0` its status becomes the script's: a caller that closed stdout,
/// or filled the device stdout pointed at, turns a green, in-budget run
/// into a failure with nothing in the log to explain it. Measured on the
/// copy before the fix:
///
/// ```text
/// $ bash -c 'bash scripts/output-budget.sh --log … --label demo -- echo hi >&-'
/// scripts/output-budget.sh: line 119: printf: write error: Bad file descriptor
/// exit=1
/// ```
///
/// The write error still reaches stderr afterwards -- it is real and is
/// not hidden. What changes is that it stops being reported as the
/// wrapped command's failure.
#[test]
fn a_verdict_that_cannot_be_written_is_not_a_failing_tier() {
    let log = log_path("closed-stdout.log");
    let output = Command::new(bash())
        .current_dir(repo())
        .arg("-c")
        .arg(format!(
            "bash scripts/output-budget.sh --log {log} --label closed -- echo hi >&-"
        ))
        .output()
        .unwrap_or_else(|error| panic!("could not run the wrapper with stdout closed: {error}"));

    assert_eq!(
        output.status.code(),
        Some(0),
        "an in-budget pass reported the verdict's own write error as the tier's status: {}",
        printed(&output)
    );
    assert_eq!(
        std::fs::read_to_string(repo().join(&log))
            .expect("the log is still written when the verdict is not")
            .trim(),
        "hi"
    );
}

#[test]
fn a_failure_keeps_the_commands_status_and_prints_its_tail_when_asked() {
    let log = log_path("failure.log");
    let output = run(
        &[
            "--log",
            &log,
            "--max-lines",
            "1",
            "--tail",
            "2",
            "--label",
            "failure",
            "--",
            "sh",
            "-c",
            "echo the-reason; exit 7",
        ],
        false,
    );

    assert_eq!(output.status.code(), Some(7), "{}", printed(&output));
    assert!(printed(&output).contains("the-reason"));
}

#[test]
fn a_failure_is_quiet_by_default_and_says_where_the_log_is() {
    let log = log_path("quiet-failure.log");
    let output = run(
        &[
            "--log",
            &log,
            "--label",
            "quiet-failure",
            "--",
            "sh",
            "-c",
            "echo the-reason; exit 7",
        ],
        false,
    );
    let terminal = printed(&output);

    assert_eq!(output.status.code(), Some(7), "{terminal}");
    assert!(
        !terminal.contains("the-reason"),
        "a failing run read its log aloud: {terminal}"
    );
    assert!(
        terminal.contains("quiet-failure: FAILED (exit 7)"),
        "the verdict did not name the status: {terminal}"
    );
    assert!(
        terminal.contains("1 lines") && terminal.contains(&log),
        "the verdict did not name the log and its size: {terminal}"
    );
}

#[test]
fn the_tail_can_be_restored_from_the_environment() {
    let log = log_path("env-tail.log");
    let output = run_with_environment(
        &[
            "--log",
            &log,
            "--label",
            "env-tail",
            "--",
            "sh",
            "-c",
            "echo the-reason; exit 7",
        ],
        false,
        &[("OUTPUT_BUDGET_FAIL_TAIL", "2")],
    );
    let terminal = printed(&output);

    assert_eq!(output.status.code(), Some(7), "{terminal}");
    assert!(
        terminal.contains("the-reason"),
        "OUTPUT_BUDGET_FAIL_TAIL did not restore the tail: {terminal}"
    );
}

#[test]
fn a_verbose_failure_does_not_repeat_the_run_it_already_streamed() {
    let log = log_path("verbose-tail.log");
    let output = run_with_environment(
        &[
            "--log",
            &log,
            "--tail",
            "40",
            "--label",
            "verbose-tail",
            "--",
            "sh",
            "-c",
            "echo the-reason; exit 7",
        ],
        true,
        &[],
    );
    let terminal = printed(&output);

    assert_eq!(output.status.code(), Some(7), "{terminal}");
    assert_eq!(
        terminal.matches("the-reason").count(),
        1,
        "the streamed run was read back a second time: {terminal}"
    );
}

#[test]
fn the_superseded_harness_variables_are_reported_rather_than_ignored() {
    let log = log_path("legacy-name.log");
    let output = run_with_environment(
        &["--log", &log, "--label", "legacy", "--", "echo", "hello"],
        false,
        &[("FLTH_VERBOSE", "1")],
    );
    let terminal = printed(&output);

    assert!(output.status.success(), "{terminal}");
    assert!(
        !terminal.contains("hello"),
        "the superseded name was honoured instead of reported: {terminal}"
    );
    assert!(
        terminal.contains("FLTH_VERBOSE") && terminal.contains("OUTPUT_BUDGET_VERBOSE"),
        "a superseded variable was ignored in silence: {terminal}"
    );
}

#[test]
fn verbose_streams_without_lifting_the_budget() {
    let log = log_path("verbose.log");
    let output = run(
        &[
            "--log",
            &log,
            "--max-lines",
            "1",
            "--",
            "sh",
            "-c",
            "printf 'one\\ntwo\\n'",
        ],
        true,
    );

    assert_eq!(output.status.code(), Some(65), "{}", printed(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("one"));
}

#[test]
fn verbose_still_returns_a_failing_commands_status() {
    let log = log_path("verbose-failure.log");
    let output = run(&["--log", &log, "--", "sh", "-c", "echo no; exit 9"], true);
    assert_eq!(output.status.code(), Some(9), "{}", printed(&output));
}

#[test]
fn the_floor_counts_all_test_binaries_and_rejects_a_short_run() {
    let directory = repo().join("tmp").join("logs");
    std::fs::create_dir_all(&directory).unwrap();
    let log = directory.join("floor-contract.log");
    std::fs::write(
        &log,
        "test result: ok. 3 passed; 0 failed\n\
         test result: ok. 4 passed; 0 failed\n",
    )
    .unwrap();

    let pass = Command::new(bash())
        .current_dir(repo())
        .args(["scripts/test-floor.sh", "floor-contract", "7"])
        .output()
        .unwrap();
    assert!(pass.status.success(), "{}", printed(&pass));

    let fail = Command::new(bash())
        .current_dir(repo())
        .args(["scripts/test-floor.sh", "floor-contract", "8"])
        .output()
        .unwrap();
    assert_eq!(fail.status.code(), Some(1), "{}", printed(&fail));
}

#[test]
fn local_tasks_and_automation_use_budgeted_tiers_and_keep_the_logs() {
    let chores = std::fs::read_to_string(repo().join("chores.yml")).unwrap();
    let ci = std::fs::read_to_string(repo().join(".github/workflows/ci.yml")).unwrap();
    let release = std::fs::read_to_string(repo().join(".github/workflows/release.yml")).unwrap();

    for expected in [
        "scripts/tier.sh \"test (debug)\" debug 750 50000 -- cargo test --locked --all-targets",
        "scripts/tier.sh \"test (release)\" release 750 50000 -- cargo test --locked --release --all-targets",
        "scripts/tier.sh coverage coverage 800 60000 -- cargo llvm-cov",
    ] {
        assert!(
            chores.contains(expected) || ci.contains(expected) || release.contains(expected),
            "the configured tier is missing: {expected}"
        );
    }
    assert!(ci.matches("actions/upload-artifact@v4").count() >= 2);
    assert!(release.contains("actions/upload-artifact@v4"));
}
