//! The gate is demonstrated by its failures, not by its passes.
//!
//! A guard that has only ever been seen to pass is indistinguishable from a
//! guard that reads nothing: both are green. Every check in this crate is
//! therefore proved here by MUTATION — take a real repository's real
//! `ci.yml` and `.github-guard`, make exactly the edit the check exists to
//! catch, and assert that check fires and names the cost.
//!
//! The configurations under `tests/fixtures/` are verbatim copies, not
//! sketches. A hand-written fixture only proves the guard handles the
//! shape its author had in mind, which is the same shape the guard was
//! written against — the two agree because they came from one head, not
//! because the rule holds. These came from:
//!
//! | fixture | repository | commit |
//! |---|---|---|
//! | `rust-fs-xfs` | antimatter-studios/rust-fs-xfs | `8dbbc57` |
//! | `rust-partitions` | antimatter-studios/rust-partitions | `01309d2` |
//! | `rust-fs-ntfs` | antimatter-studios/rust-fs-ntfs | `2889a64` |
//!
//! xfs and partitions are the two the mutations run against: they differ in
//! job count (six and four), in matrix shape, in whether a darwin runner is
//! present, and partitions carries a third workflow (`fuzz.yml`) that xfs
//! does not. ntfs is here because it is the only repository in the family
//! with genuinely non-gating jobs, and so the only real configuration that
//! exercises the per-repo exemption.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use am_ci_guard::{AggregateGate, Failure};

const XFS: &str = "rust-fs-xfs";
const PARTITIONS: &str = "rust-partitions";
const NTFS: &str = "rust-fs-ntfs";

/// Both repositories the mutations run against.
const MUTATED: [&str; 2] = [XFS, PARTITIONS];

/// ntfs's real required list: the five named check names it declares
/// instead of an aggregate. Replaced wholesale where a test gives it the
/// aggregate it does not have yet -- swapping only the first line would
/// leave four more required checks behind, and the test would be asserting
/// something other than what it says.
const NTFS_REQUIRED: &str = "\trequired = test-ubuntu-latest\n\
     \trequired = test-ubuntu-24.04-arm\n\
     \trequired = test-macos-latest\n\
     \trequired = full test suite (fixtures + integration)\n\
     \trequired = validate rust-ntfs format (Windows chkdsk)";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// A throwaway copy of a fixture repository, mutable and self-deleting.
struct Scratch(PathBuf);

impl Scratch {
    /// Copy `fixture`'s `ci.yml` and `.github-guard` somewhere writable.
    fn of(fixture: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "am-ci-guard-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            fixture
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".github").join("workflows")).expect("scratch dir");
        let from = fixtures().join(fixture);
        for rel in [
            PathBuf::from(".github-guard"),
            PathBuf::from(".github").join("workflows").join("ci.yml"),
        ] {
            std::fs::copy(from.join(&rel), dir.join(&rel))
                .unwrap_or_else(|e| panic!("copy {}: {e}", rel.display()));
        }
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    /// Read, with CRLF normalised to LF.
    ///
    /// This job also runs on windows-latest, where git may check the
    /// fixtures out with CRLF. Every mutation below is written with `\n`,
    /// and against a CRLF checkout each one would silently fail to
    /// match -- which `mutate` turns into a loud failure rather than a
    /// pass, but a failure about the wrong thing. `.gitattributes` pins
    /// the fixtures to LF as well; this is the half that does not depend
    /// on anyone's checkout being configured correctly.
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel))
            .expect(rel)
            .replace("\r\n", "\n")
    }

    /// Replace the first occurrence of `from` with `to`, refusing if it is
    /// not there. A mutation that did not apply proves nothing, and a
    /// silent no-op would leave this file asserting that an UNCHANGED
    /// config fails — the exact inversion it exists to rule out.
    fn mutate(&self, rel: &str, from: &str, to: &str) -> &Self {
        let text = self.read(rel);
        assert!(
            text.contains(from),
            "the mutation does not apply: {rel} in this fixture has no {from:?}. \
             The fixture has moved on and this test is no longer mutating anything."
        );
        std::fs::write(self.path(rel), text.replacen(from, to, 1)).expect(rel);
        self
    }

    fn gate(&self) -> AggregateGate {
        AggregateGate::new(&self.0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Assert that exactly the named check fired, and that its message says
/// what it cost. Checking the message is not decoration: a check that
/// fires with an unhelpful message is a check whose failure gets silenced
/// rather than fixed.
#[track_caller]
fn fires(failures: &[Failure], check: &str, needle: &str) {
    let hit: Vec<_> = failures.iter().filter(|f| f.check == check).collect();
    assert!(
        !hit.is_empty(),
        "expected `{check}` to fire; what fired instead: {failures:#?}"
    );
    assert!(
        hit.iter().any(|f| f.detail.contains(needle)),
        "`{check}` fired but did not explain {needle:?}: {hit:#?}"
    );
}

// ---------------------------------------------------------------------
// The baseline. Everything below is only meaningful because of this.
// ---------------------------------------------------------------------

/// Every real configuration passes unmodified.
///
/// Without this the mutation tests would also pass against a guard that
/// fails on absolutely everything.
#[test]
fn the_real_configurations_pass_unmodified() {
    for fixture in MUTATED {
        let scratch = Scratch::of(fixture);
        let failures = scratch.gate().check();
        assert!(
            failures.is_empty(),
            "{fixture} gates correctly today and the guard should say so: {failures:#?}"
        );
    }
}

// ---------------------------------------------------------------------
// Mutation 1 — a job drops out of `needs:`.
// ---------------------------------------------------------------------

/// The job is still in `ci.yml`, still runs, still reports. It just no
/// longer gates, and nothing else in either repository would notice.
#[test]
fn removing_a_job_from_needs_fires() {
    // xfs: six jobs behind the gate. Drop the macOS leg -- the one whose
    // absence is least visible, because the other five still run.
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        "needs: [unit, fixtures, test, test-arm64, suite-in-vm]",
    );
    fires(
        &xfs.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "`ci-ok` does not need `test-darwin`, so that job gates nothing",
    );

    // partitions: four jobs, and a different one -- the oracle suite that
    // checks this crate's tables against sgdisk, sfdisk, blkid and partx.
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github/workflows/ci.yml",
        "needs: [test, test-release, oracle, fmt]",
        "needs: [test, test-release, fmt]",
    );
    fires(
        &partitions.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "`ci-ok` does not need `oracle`, so that job gates nothing",
    );
}

/// `needs:` emptied entirely: the aggregate reports success having asked
/// nobody. This is the failure that looks most like success.
#[test]
fn an_aggregate_that_needs_nothing_fires() {
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        "needs: []",
    );
    fires(
        &xfs.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "needs nothing, so it says every job succeeded while asking none of them",
    );
}

/// A `needs:` entry naming a job that is not there. GitHub refuses to run
/// the workflow at all, so the one required check never reports — and a
/// required check that never reports is a permanent block on every merge.
#[test]
fn needing_a_job_that_does_not_exist_fires() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github/workflows/ci.yml",
        "needs: [test, test-release, oracle, fmt]",
        "needs: [test, test-release, oracle, fmt, clippy]",
    );
    fires(
        &partitions.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "`ci-ok` needs `clippy`, which is not a job in",
    );
}

// ---------------------------------------------------------------------
// Mutation 2 — `if: always()` comes off the aggregate.
// ---------------------------------------------------------------------

/// Without it the aggregate is skipped whenever anything it needs was
/// cancelled or skipped, and a skipped required check never reports.
#[test]
fn deleting_if_always_fires() {
    // xfs writes it on the line before `needs:`, so mutate the pair to
    // avoid catching one of the five step-level `if: always()` above it.
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "    if: always()\n    needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        "    needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
    );
    fires(
        &xfs.gate().check(),
        "the_aggregate_runs_whatever_happened",
        "a skipped required check never reports",
    );

    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github/workflows/ci.yml",
        "    if: always()\n    needs: [test, test-release, oracle, fmt]",
        "    needs: [test, test-release, oracle, fmt]",
    );
    fires(
        &partitions.gate().check(),
        "the_aggregate_runs_whatever_happened",
        "a skipped required check never reports",
    );
}

/// `always()` narrowed to something that can be false. This is the subtler
/// edit — the key is still there, so a reader skimming the diff sees
/// `if:` where they expected `if:` — and the original line-scanning copies
/// in all eleven repositories accepted it, because they looked for the
/// exact text `if: always()` and reported only its absence.
#[test]
fn narrowing_always_to_a_condition_that_can_be_false_fires() {
    for (fixture, needs) in [
        (
            XFS,
            "needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        ),
        (PARTITIONS, "needs: [test, test-release, oracle, fmt]"),
    ] {
        let scratch = Scratch::of(fixture);
        scratch.mutate(
            ".github/workflows/ci.yml",
            &format!("    if: always()\n    {needs}"),
            &format!("    if: always() && github.event_name == 'pull_request'\n    {needs}"),
        );
        fires(
            &scratch.gate().check(),
            "the_aggregate_runs_whatever_happened",
            "Any condition that can be false is a condition under which the one \
             required check does not report",
        );
    }
}

/// `${{ always() }}` is the same expression with braces, and must pass.
/// A guard that rejects a correct spelling gets worked around.
#[test]
fn the_braced_spelling_of_always_passes() {
    for (fixture, needs) in [
        (
            XFS,
            "needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        ),
        (PARTITIONS, "needs: [test, test-release, oracle, fmt]"),
    ] {
        let scratch = Scratch::of(fixture);
        scratch.mutate(
            ".github/workflows/ci.yml",
            &format!("    if: always()\n    {needs}"),
            &format!("    if: ${{{{ always() }}}}\n    {needs}"),
        );
        let failures = scratch.gate().check();
        assert!(
            failures.is_empty(),
            "{fixture}: `${{{{ always() }}}}` is `always()`: {failures:#?}"
        );
    }
}

// ---------------------------------------------------------------------
// Mutation 3 — `.github-guard` requires something else as well.
// ---------------------------------------------------------------------

/// A second required check re-introduces the whole problem: that name is
/// now pinned in a file the pull request renaming it cannot also change,
/// because github-guard reads `.github-guard` from the DEFAULT BRANCH.
#[test]
fn a_second_required_check_fires() {
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github-guard",
        "\trequired = ci-ok",
        "\trequired = ci-ok\n\trequired = test / ubuntu-latest",
    );
    fires(
        &xfs.gate().check(),
        "protection_requires_the_aggregate_and_nothing_else",
        r#"requires ["ci-ok", "test / ubuntu-latest"]"#,
    );

    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github-guard",
        "\trequired = ci-ok",
        "\trequired = ci-ok\n\trequired = fmt",
    );
    fires(
        &partitions.gate().check(),
        "protection_requires_the_aggregate_and_nothing_else",
        r#"requires ["ci-ok", "fmt"]"#,
    );
}

/// The aggregate dropped from the required list entirely: every job still
/// runs, and none of them is required.
#[test]
fn requiring_something_other_than_the_aggregate_fires() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(".github-guard", "\trequired = ci-ok", "\trequired = fmt");
    fires(
        &partitions.gate().check(),
        "protection_requires_the_aggregate_and_nothing_else",
        r#"requires ["fmt"]; it should name `ci-ok` alone"#,
    );
}

/// The prose is not the declaration. Every one of these files opens with a
/// long argued rationale, and partitions' rationale contains the literal
/// text `required` beside old check names. A scanner that takes any line
/// containing `required =` reads an argument about what used to be
/// required as a list of things that are required.
#[test]
fn a_required_line_inside_a_comment_is_not_a_requirement() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github-guard",
        "[checks]",
        "# a note about how this used to say `required = test / ubuntu-latest`\n[checks]",
    );
    let failures = partitions.gate().check();
    assert!(
        failures.is_empty(),
        "a comment is not a declaration: {failures:#?}"
    );
}

// ---------------------------------------------------------------------
// Mutation 4 — the gate workflow stops gating pull requests.
// ---------------------------------------------------------------------

/// Only a `pull_request` workflow can gate a pull request. None of the
/// eleven copied guards checked this, and it is the premise every one of
/// them rests on.
#[test]
fn a_gate_workflow_that_does_not_run_on_pull_requests_fires() {
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "  pull_request:",
        "  workflow_dispatch:",
    );
    fires(
        &xfs.gate().check(),
        "the_gate_workflow_runs_on_pull_request",
        "which GitHub reads as permanently pending",
    );
}

/// partitions' real `fuzz.yml` — `workflow_dispatch` plus a nightly cron —
/// pointed at as the gate. It reports on no pull request ever, so
/// requiring anything from it blocks every merge forever. This is the
/// mistake the check exists to make impossible, on a real file.
#[test]
fn pointing_the_gate_at_the_nightly_fuzz_workflow_fires() {
    let scratch = Scratch::of(PARTITIONS);
    std::fs::copy(
        fixtures()
            .join(PARTITIONS)
            .join(".github")
            .join("workflows")
            .join("fuzz.yml"),
        scratch.path(".github/workflows/fuzz.yml"),
    )
    .expect("fuzz.yml");
    let failures = scratch
        .gate()
        .workflow(".github/workflows/fuzz.yml")
        .check();
    fires(
        &failures,
        "the_gate_workflow_runs_on_pull_request",
        "does not run on `pull_request`",
    );
}

// ---------------------------------------------------------------------
// The per-repo exemption, on the only real configuration that has one.
// ---------------------------------------------------------------------

/// rust-fs-ntfs has no aggregate job at all — its `.github-guard` names
/// five real check names and argues, at #278, that the aggregate is the
/// better end state it has not adopted yet. The guard has to say so
/// plainly rather than pass by finding nothing to complain about.
#[test]
fn a_repository_with_no_aggregate_job_fires() {
    let ntfs = Scratch::of(NTFS);
    fires(
        &ntfs.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "has no `ci-ok` job, so protection has to name every job by hand",
    );
}

/// ntfs's real jobs, given the aggregate it does not have yet.
///
/// `changes` carries `if: github.event_name == 'pull_request'` and
/// `validate-mkfs-windows` an `if:` restricting it to tag pushes and
/// `workflow_dispatch`. Both are deliberately conditional. Needing them
/// anyway means every pull request fails on a job that was never meant to
/// run on one — the aggregate counts `skipped` as not-success, which is
/// precisely what makes it worth having.
#[test]
fn needing_a_conditional_job_fires() {
    let ntfs = Scratch::of(NTFS);
    ntfs.mutate(
        ".github/workflows/ci.yml",
        "jobs:\n",
        "jobs:\n  ci-ok:\n    if: always()\n    needs: [test, integration, asan, changes, \
         validate-mkfs-windows]\n    runs-on: ubuntu-latest\n    steps:\n      - run: true\n",
    );
    ntfs.mutate(".github-guard", NTFS_REQUIRED, "\trequired = ci-ok");
    let failures = ntfs.gate().check();
    for job in ["changes", "validate-mkfs-windows"] {
        fires(
            &failures,
            "the_aggregate_job_needs_every_other_job",
            &format!("`{job}` carries [\"if\"] but is not declared non-gating"),
        );
    }
}

/// Declaring them non-gating and taking them out of `needs:` is the fix,
/// and the guard has to accept it — otherwise the only way past is to
/// delete the guard.
#[test]
fn declaring_the_conditional_jobs_non_gating_passes() {
    let ntfs = Scratch::of(NTFS);
    ntfs.mutate(
        ".github/workflows/ci.yml",
        "jobs:\n",
        "jobs:\n  ci-ok:\n    if: always()\n    needs: [test, integration, asan]\n    \
         runs-on: ubuntu-latest\n    steps:\n      - run: true\n",
    );
    ntfs.mutate(".github-guard", NTFS_REQUIRED, "\trequired = ci-ok");
    let failures = ntfs
        .gate()
        .non_gating(["changes", "validate-mkfs-windows"])
        .check();
    assert!(
        failures.is_empty(),
        "a declared, genuinely conditional job is the supported shape: {failures:#?}"
    );
}

/// The exemption list cannot drift away from the workflow. Declaring a job
/// non-gating when it carries no conditional key takes a working gate off
/// a job whose failures are real — which is how an exemption written for
/// one situation outlives it.
#[test]
fn exempting_a_job_that_is_not_conditional_fires() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github/workflows/ci.yml",
        "needs: [test, test-release, oracle, fmt]",
        "needs: [test, test-release, fmt]",
    );
    fires(
        &partitions.gate().non_gating(["oracle"]).check(),
        "the_aggregate_job_needs_every_other_job",
        "`oracle` is declared non-gating but carries none of",
    );
}

/// And an exemption for a job that does not exist at all.
#[test]
fn exempting_a_job_that_is_not_there_fires() {
    let xfs = Scratch::of(XFS);
    fires(
        &xfs.gate().non_gating(["asan"]).check(),
        "the_aggregate_job_needs_every_other_job",
        "`asan` is declared non-gating but is not a job in",
    );
}

/// A `continue-on-error: true` job inside `needs:` is the quietest hole of
/// the lot: the job reports `success` however it ended, so the aggregate
/// reads a green tick for a red job, and both halves of the old guard pass.
#[test]
fn a_continue_on_error_job_inside_needs_fires() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(
        ".github/workflows/ci.yml",
        "  oracle:\n",
        "  oracle:\n    continue-on-error: true\n",
    );
    fires(
        &partitions.gate().check(),
        "the_aggregate_job_needs_every_other_job",
        "`oracle` carries [\"continue-on-error\"] but is not declared non-gating",
    );
}

// ---------------------------------------------------------------------
// The files the guard reads have to be there.
// ---------------------------------------------------------------------

/// A guard that cannot read its input has not passed.
#[test]
fn a_missing_input_fires_rather_than_passing() {
    let xfs = Scratch::of(XFS);
    std::fs::remove_file(xfs.path(".github-guard")).expect("remove");
    fires(
        &xfs.gate().check(),
        "protection_requires_the_aggregate_and_nothing_else",
        "nothing in the repository knows what it says",
    );

    let partitions = Scratch::of(PARTITIONS);
    std::fs::remove_file(partitions.path(".github/workflows/ci.yml")).expect("remove");
    fires(
        &partitions.gate().check(),
        "the_gate_workflow_exists",
        "there is nothing requiring anything",
    );
}

/// Unparseable YAML is a failure, never a pass. The line-scanning copies
/// this crate replaces had no way to tell the two apart: a file they could
/// not make sense of simply yielded no jobs, and no jobs means nothing to
/// complain about.
#[test]
fn a_workflow_that_does_not_parse_fires() {
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "jobs:\n",
        "jobs:\n  - [unclosed\n",
    );
    fires(
        &xfs.gate().check(),
        "the_gate_workflow_exists",
        "a file it cannot parse is a failure and never a pass",
    );
}

/// `verify()` is what a consumer calls, and it has to panic — a failure
/// that returns a value the caller can ignore is not a gate.
#[test]
#[should_panic(expected = "the CI gate is not holding")]
fn verify_panics_on_a_broken_configuration() {
    let partitions = Scratch::of(PARTITIONS);
    partitions.mutate(".github-guard", "\trequired = ci-ok", "\trequired = fmt");
    partitions.gate().verify();
}

/// And `verify()` reports every failure at once. A repository being
/// brought onto the gate wants the whole list in one run, not one per push.
#[test]
fn every_failure_is_reported_together() {
    let xfs = Scratch::of(XFS);
    xfs.mutate(
        ".github/workflows/ci.yml",
        "    if: always()\n    needs: [unit, fixtures, test, test-arm64, test-darwin, suite-in-vm]",
        "    needs: [unit, fixtures, test, test-arm64, suite-in-vm]",
    );
    xfs.mutate(".github-guard", "\trequired = ci-ok", "\trequired = fmt");
    let failures = xfs.gate().check();
    let checks: Vec<_> = failures.iter().map(|f| f.check).collect();
    for expected in [
        "the_aggregate_job_needs_every_other_job",
        "the_aggregate_runs_whatever_happened",
        "protection_requires_the_aggregate_and_nothing_else",
    ] {
        assert!(
            checks.contains(&expected),
            "three separate things are wrong and only {checks:?} was reported"
        );
    }
}
