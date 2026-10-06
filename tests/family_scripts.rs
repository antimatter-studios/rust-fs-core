//! The family's shared scripts, run the way every other repository runs them.
//!
//! `scripts/test-floor.sh` and `scripts/semver-check.sh` lived as a separate
//! copy in each repository -- twelve of the first and thirteen of the second
//! -- and the copies drifted: one bug (a floor that exited silently on a tier
//! that ran nothing) had to be fixed seven times, and a counting rule one
//! repository added reached no other. They are now rust-fs-core's alone,
//! exactly as `scripts/output-budget.sh` already was: a repository carries one
//! byte-identical bootstrap, `scripts/core.sh`, which finds this crate and runs
//! the script named against the calling repository.
//!
//! So everything here runs the scripts from a CALLER tree -- a repository that
//! is not this one -- because that is the only way they are used. Each caller
//! tree is built under this repository's `tmp/`, never the OS temporary
//! directory, and removed afterwards.
//!
//! THE SCRIPTS RUN IN PLACE (#212). A caller used to carry a byte-identical
//! bootstrap, `scripts/core.sh`, and fourteen of them a `scripts/tier.sh`, each
//! there only to find this crate; the copies had to be recopied on every bump,
//! and the tier wrappers drifted fourteen ways. Now a caller runs
//! `bash ../rust-fs-core/scripts/NAME.sh` from its own checkout, the script
//! works on the repository it is run from, and family-check refuses a copy of
//! either file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bash() -> PathBuf {
    if cfg!(windows) {
        for candidate in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return path;
            }
        }
    }
    PathBuf::from("bash")
}

fn printed(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A throwaway repository under this one's `tmp/`, removed on drop: a git
/// repository of its own, so a script run from it treats it as the caller,
/// with a log directory and no copy of anything of this crate's.
struct Caller {
    root: PathBuf,
}

impl Caller {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = repo().join("tmp").join(format!(
            "family-caller-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        // The guard exists before anything that can panic, so a failed setup
        // still removes the tree: a panic before `Caller` was built would
        // skip its Drop and leave the directory behind.
        let caller = Caller { root };
        fs::create_dir_all(caller.root.join("scripts")).unwrap();
        fs::create_dir_all(caller.root.join("tmp").join("logs")).unwrap();
        fs::write(
            caller.root.join("Cargo.toml"),
            "[package]\nname = \"am-example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let out = Command::new("git")
            .current_dir(&caller.root)
            .args(["init", "-q"])
            .output()
            .expect("git is needed to build a caller repository");
        assert!(out.status.success(), "git init: {}", printed(&out));
        caller
    }

    fn log(&self, tier: &str, text: &str) {
        fs::write(
            self.root
                .join("tmp")
                .join("logs")
                .join(format!("{tier}.log")),
            text,
        )
        .unwrap();
    }

    /// `bash <this crate>/scripts/NAME.sh ARGS...` from the caller's own
    /// directory, with nothing set to say which repository it is: the script
    /// finds that from where it is run, as a consumer's CI runs it.
    fn core(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let (name, rest) = args.split_first().expect("a script name");
        let mut command = Command::new(bash());
        command
            .current_dir(&self.root)
            .arg(repo().join("scripts").join(format!("{name}.sh")))
            .args(rest)
            .env_remove("FS_CORE_CALLER")
            .env_remove("FS_CORE_ROOT");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn version_of(script: &str) -> String {
    let out = Command::new(bash())
        .arg(repo().join("scripts").join(script))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn each_family_script_states_its_contract() {
    assert_eq!(version_of("test-floor.sh"), "rust-fs-core-test-floor 1");
    assert_eq!(version_of("semver-check.sh"), "rust-fs-core-semver-check 1");
    assert_eq!(
        version_of("guest-rust-toolchain.sh"),
        "rust-fs-core-guest-rust-toolchain 1"
    );
    assert_eq!(
        version_of("stage-siblings.sh"),
        "rust-fs-core-stage-siblings 1"
    );
    assert_eq!(
        version_of("guest-rust-run.sh"),
        "rust-fs-core-guest-rust-run 1"
    );
    assert_eq!(version_of("ci-gate.sh"), "rust-fs-core-ci-gate 1");
    assert_eq!(version_of("package-cli.sh"), "rust-fs-core-package-cli 1");
}

/// `ci-gate` runs in place like the others. Its behaviour against
/// a caller's `ci.yml` and `.github-guard` is `tests/scripts/test-ci-gate.sh`,
/// which needs python3's yaml module and so runs where CI provides one.
#[test]
fn ci_gate_runs_in_place() {
    let caller = Caller::new("ci-gate");
    let out = caller.core(&["ci-gate", "--version"], &[]);
    assert!(out.status.success(), "{}", printed(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rust-fs-core-ci-gate 1",
        "{}",
        printed(&out)
    );
}

#[test]
fn the_floor_reads_the_callers_log_not_this_repositorys() {
    let caller = Caller::new("floor");
    caller.log(
        "unit",
        "test result: ok. 3 passed; 0 failed\ntest result: ok. 4 passed; 0 failed\n",
    );
    let pass = caller.core(&["test-floor", "unit", "7"], &[]);
    assert!(pass.status.success(), "{}", printed(&pass));
    assert!(
        String::from_utf8_lossy(&pass.stdout).contains("unit: 7 tests executed (floor 7)"),
        "{}",
        printed(&pass)
    );

    let fail = caller.core(&["test-floor", "unit", "8"], &[]);
    assert_eq!(fail.status.code(), Some(1), "{}", printed(&fail));
    assert!(
        String::from_utf8_lossy(&fail.stdout)
            .contains("only 7 tests executed in the unit tier, floor is 8"),
        "{}",
        printed(&fail)
    );
}

#[test]
fn the_floor_names_a_tier_that_ran_nothing_and_one_that_never_ran() {
    let caller = Caller::new("empty");
    caller.log("empty", "compiled; no test binary\n");
    let empty = caller.core(&["test-floor", "empty", "1"], &[]);
    assert_eq!(empty.status.code(), Some(1), "{}", printed(&empty));
    assert!(
        String::from_utf8_lossy(&empty.stdout)
            .contains("only 0 tests executed in the empty tier, floor is 1"),
        "{}",
        printed(&empty)
    );

    let absent = caller.core(&["test-floor", "never-ran", "1"], &[]);
    assert_eq!(absent.status.code(), Some(1), "{}", printed(&absent));
    assert!(
        String::from_utf8_lossy(&absent.stderr).contains("did not run"),
        "{}",
        printed(&absent)
    );
}

#[test]
fn the_floor_counts_a_semver_runs_lints_coloured_or_not() {
    let caller = Caller::new("semver-lints");
    caller.log(
        "semver",
        "semver-check: am-example 0.1.0\n\u{1b}[1m\u{1b}[32m     Checked\u{1b}[0m [   0.013s] 196 checks: 196 pass, 58 skip\n",
    );
    let out = caller.core(&["test-floor", "semver", "196"], &[]);
    assert!(out.status.success(), "{}", printed(&out));
    let short = caller.core(&["test-floor", "semver", "197"], &[]);
    assert_eq!(short.status.code(), Some(1), "{}", printed(&short));
}

#[test]
fn refusing_ignored_tests_is_asked_for_and_then_enforced() {
    let caller = Caller::new("ignored");
    caller.log(
        "unit",
        "test result: ok. 5 passed; 0 failed; 2 ignored; 0 measured\n",
    );
    let allowed = caller.core(&["test-floor", "unit", "5"], &[]);
    assert!(allowed.status.success(), "{}", printed(&allowed));
    let refused = caller.core(&["test-floor", "--refuse-ignored", "unit", "5"], &[]);
    assert_eq!(refused.status.code(), Some(1), "{}", printed(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stdout).contains("2 tests ignored in the unit tier"),
        "{}",
        printed(&refused)
    );
}

#[test]
fn semver_check_checks_the_callers_package() {
    let caller = Caller::new("semver");
    let out = caller.core(&["semver-check"], &[("SEMVER_CHECK_DRY_RUN", "1")]);
    assert!(out.status.success(), "{}", printed(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("cargo semver-checks check-release --package am-example"),
        "{}",
        printed(&out)
    );
    assert!(
        // The package flag, not the bare name: the checkout's own path is
        // `.../rust-fs-core/...`, so the name alone is in every line.
        !said.contains("--package rust-fs-core") && !said.contains("--package am-fs-core"),
        "it checked this crate, not the caller's: {}",
        printed(&out)
    );
}

#[test]
fn semver_check_names_the_former_name_a_renamed_crate_is_compared_with() {
    let caller = Caller::new("semver-former");
    fs::write(
        caller.root.join("Cargo.toml"),
        "[package]\nname = \"rust-example\"\nversion = \"0.2.0\"\n\n\
         [package.metadata.semver]\nformer-name = \"am-example\"\n",
    )
    .unwrap();
    let out = caller.core(&["semver-check"], &[("SEMVER_CHECK_DRY_RUN", "1")]);
    assert!(out.status.success(), "{}", printed(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("--package rust-example"),
        "it did not check the crate under its new name: {}",
        printed(&out)
    );
    assert!(
        said.contains("against am-example's newest release"),
        "it did not say the former name is the baseline: {}",
        printed(&out)
    );
}

/// A script run in place works on the repository it is run from, and a
/// script run from somewhere that is no repository at all says so rather
/// than reading this crate's own files as if they were the caller's.
#[test]
fn a_script_run_in_place_works_on_the_repository_it_is_run_from() {
    let caller = Caller::new("in-place");
    caller.log("unit", "test result: ok. 2 passed; 0 failed\n");
    let out = caller.core(&["test-floor", "unit", "2"], &[]);
    assert!(out.status.success(), "{}", printed(&out));

    // A subdirectory of the caller is still the caller.
    let nested = caller.root.join("src");
    fs::create_dir_all(&nested).unwrap();
    let out = Command::new(bash())
        .current_dir(&nested)
        .arg(repo().join("scripts").join("test-floor.sh"))
        .args(["unit", "2"])
        .env_remove("FS_CORE_CALLER")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));

    // FS_CORE_CALLER still names the caller outright, wherever it is run.
    let elsewhere = Command::new(bash())
        .current_dir(repo())
        .arg(repo().join("scripts").join("test-floor.sh"))
        .args(["unit", "2"])
        .env("FS_CORE_CALLER", &caller.root)
        .output()
        .unwrap();
    assert!(elsewhere.status.success(), "{}", printed(&elsewhere));
}

#[test]
fn this_repository_runs_its_own_scripts_in_place_too() {
    let out = Command::new(bash())
        .current_dir(repo())
        .arg(repo().join("scripts").join("test-floor.sh"))
        .arg("--version")
        .env_remove("FS_CORE_ROOT")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "rust-fs-core-test-floor 1"
    );
}

/// The published crate is how a consumer with no checkout gets these, so the
/// package must carry all three.
#[test]
fn the_package_ships_every_family_script() {
    let manifest = fs::read_to_string(repo().join("Cargo.toml")).unwrap();
    for line in manifest.lines() {
        let line = line.trim();
        assert!(
            !(line.starts_with("include")
                || (line.starts_with("exclude") && line.contains("scripts"))),
            "Cargo.toml narrows the package in a way that could drop scripts/: {line}"
        );
    }
    for script in [
        "core.sh",
        "output-budget.sh",
        "test-floor.sh",
        "semver-check.sh",
        "family-check.sh",
        "guest-rust-toolchain.sh",
        "ci-gate.sh",
        "package-cli.sh",
        "stage-siblings.sh",
        "guest-rust-run.sh",
        "tier.sh",
        "release-notes.sh",
        "changelog-draft.sh",
    ] {
        assert!(
            Path::new(&repo().join("scripts").join(script)).is_file(),
            "scripts/{script}"
        );
    }
}

#[test]
fn family_check_passes_a_clean_caller_and_refuses_each_kind_of_copy() {
    // Clean: the scripts are run in place from the rust-fs-core checkout.
    let clean = Caller::new("family-clean");
    fs::write(
        clean.root.join("chores.yml"),
        "tasks:\n  t:\n    cmds:\n      - 'bash ../rust-fs-core/scripts/test-floor.sh debug 1'\n",
    )
    .unwrap();
    let ok = clean.core(&["family-check"], &[]);
    assert!(ok.status.success(), "{}", printed(&ok));
    assert!(
        String::from_utf8_lossy(&ok.stdout).contains("PASS  family-check"),
        "{}",
        printed(&ok)
    );

    // A committed copy of a family script.
    let copy = Caller::new("family-copy");
    fs::write(copy.root.join("scripts/test-floor.sh"), "echo mine\n").unwrap();
    let out = copy.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(
            "scripts/test-floor.sh is a copy of rust-fs-core's; delete it and run ../rust-fs-core/scripts/test-floor.sh in place"
        ),
        "{}",
        printed(&out)
    );

    // The bootstrap and the tier wrapper are copies too: each existed only to
    // find this crate, and both drifted (#212).
    for wrapper in ["core.sh", "tier.sh"] {
        let kept = Caller::new("family-wrapper");
        fs::write(kept.root.join("scripts").join(wrapper), "echo mine\n").unwrap();
        let out = kept.core(&["family-check"], &[]);
        assert_eq!(out.status.code(), Some(1), "{wrapper}: {}", printed(&out));
        assert!(
            String::from_utf8_lossy(&out.stdout)
                .contains(&format!("scripts/{wrapper} is a copy of rust-fs-core's")),
            "{wrapper}: {}",
            printed(&out)
        );
    }

    // A call through a copy, or through the old bootstrap or tier wrapper,
    // is refused; a comment naming one is fine.
    let calls = Caller::new("family-calls");
    fs::create_dir_all(calls.root.join(".github/workflows")).unwrap();
    fs::write(
        calls.root.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      # see scripts/semver-check.sh\n      - run: bash scripts/test-floor.sh debug 1\n      - run: bash scripts/core.sh test-floor debug 1\n      - run: scripts/tier.sh unit unit 10 100 -- cargo test\n",
    )
    .unwrap();
    let out = calls.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "bash scripts/test-floor.sh debug 1",
        "bash scripts/core.sh test-floor debug 1",
        "scripts/tier.sh unit unit 10 100",
    ] {
        assert!(said.contains(expected), "{expected}: {}", printed(&out));
    }
    assert!(
        !said.contains("see scripts/semver-check.sh"),
        "a comment was counted as a call: {}",
        printed(&out)
    );

    // A driver's own guest command is its scripts/guest-suite.sh, and the
    // family script it runs is run in place from the staged sibling.
    let driver = Caller::new("family-guest-suite-is-the-drivers");
    fs::write(
        driver.root.join("scripts/guest-suite.sh"),
        "exec bash /share/siblings/rust-fs-core/scripts/guest-rust-run.sh fs-x /share rust-fs-core -- scripts/test.sh\n",
    )
    .unwrap();
    fs::write(
        driver.root.join("chores.yml"),
        "tasks:\n  test:vm:\n    cmds:\n      - 'bash ../rust-fs-core/scripts/stage-siblings.sh /share rust-fs-core -- true'\n",
    )
    .unwrap();
    let out = driver.core(&["family-check"], &[]);
    assert!(out.status.success(), "{}", printed(&out));
}

/// The bootstrap is refused whatever its mode: it used to be required, and
/// executable (rust-fs-core#200); it is now a copy like any other (#212).
#[test]
fn family_check_refuses_a_committed_bootstrap_however_it_is_kept() {
    let git = |caller: &Caller, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&caller.root)
            .args(args)
            .output()
            .expect("git is needed to build a caller repository");
        assert!(out.status.success(), "git {args:?}: {}", printed(&out));
    };
    let caller = Caller::new("family-bootstrap-committed");
    fs::copy(
        repo().join("scripts").join("core.sh"),
        caller.root.join("scripts").join("core.sh"),
    )
    .unwrap();
    git(&caller, &["add", "scripts/core.sh"]);
    git(&caller, &["update-index", "--chmod=+x", "scripts/core.sh"]);
    let out = caller.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("scripts/core.sh is a copy of rust-fs-core's"),
        "{}",
        printed(&out)
    );
}

/// One tier runner for the family (#212): run in place, it writes the log
/// under the caller's `tmp/logs/`, not this crate's.
#[test]
fn the_tier_runner_runs_in_place_and_logs_in_the_caller() {
    let caller = Caller::new("tier");
    let out = caller.core(
        &[
            "tier",
            "unit",
            "unit",
            "50",
            "5000",
            "--",
            "echo",
            "hello from the caller",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", printed(&out));
    let log = fs::read_to_string(caller.root.join("tmp/logs/unit.log"))
        .expect("the tier's log is in the caller's tmp/logs");
    assert!(log.contains("hello from the caller"), "{log}");
}

/// The check three repositories carried in their own tier wrappers, shared:
/// a run that printed SKIP lines counted them as passes, and a skipped test
/// reads exactly like a passing one (#212).
#[test]
fn the_tier_runner_refuses_a_skip_when_asked() {
    let caller = Caller::new("tier-skips");
    let refused = caller.core(
        &[
            "tier",
            "--refuse-skips",
            "unit",
            "unit",
            "50",
            "5000",
            "--",
            "echo",
            "SKIP: no fixture",
        ],
        &[],
    );
    assert_eq!(refused.status.code(), Some(66), "{}", printed(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("SKIP"),
        "the refusal does not name the skip: {}",
        printed(&refused)
    );

    let allowed = caller.core(
        &[
            "tier",
            "unit",
            "unit",
            "50",
            "5000",
            "--",
            "echo",
            "SKIP: no fixture",
        ],
        &[],
    );
    assert!(
        allowed.status.success(),
        "a skip is only refused when asked: {}",
        printed(&allowed)
    );
}

#[test]
fn per_target_floors_catch_a_suite_that_emptied_inside_a_healthy_total() {
    let caller = Caller::new("targets");
    fs::create_dir_all(caller.root.join(".github")).unwrap();
    fs::write(
        caller.root.join(".github/test-floors.txt"),
        "# target floor\nlib 2\nreads 3\nwrites 1\n",
    )
    .unwrap();
    // Coloured `Running` lines, as CI's CARGO_TERM_COLOR=always prints them.
    let run = |writes: u32| {
        format!(
            "\u{1b}[1m\u{1b}[32m     Running\u{1b}[0m unittests src/lib.rs (x)\ntest result: ok. 2 passed\n\
             \u{1b}[1m\u{1b}[32m     Running\u{1b}[0m tests/reads.rs (x)\ntest result: ok. 9 passed\n\
             \u{1b}[1m\u{1b}[32m     Running\u{1b}[0m tests/writes.rs (x)\ntest result: ok. {writes} passed\n"
        )
    };
    caller.log("suite", &run(1));
    let ok = caller.core(
        &[
            "test-floor",
            "--targets",
            ".github/test-floors.txt",
            "suite",
        ],
        &[],
    );
    assert!(ok.status.success(), "{}", printed(&ok));
    assert!(
        String::from_utf8_lossy(&ok.stdout).contains("3 target(s) met their floors"),
        "{}",
        printed(&ok)
    );

    // A target the log never mentions -- a suite that stopped being BUILT --
    // counts as zero, not as absent and therefore fine.
    fs::write(
        caller.root.join(".github/test-floors.txt"),
        "reads 3\nnever_built 1\n",
    )
    .unwrap();
    caller.log("suite", &run(1));
    let unbuilt = caller.core(
        &[
            "test-floor",
            "--targets",
            ".github/test-floors.txt",
            "suite",
        ],
        &[],
    );
    assert_eq!(unbuilt.status.code(), Some(1), "{}", printed(&unbuilt));
    assert!(
        String::from_utf8_lossy(&unbuilt.stdout)
            .contains("never_built executed 0 tests, floor is 1"),
        "{}",
        printed(&unbuilt)
    );
    fs::write(
        caller.root.join(".github/test-floors.txt"),
        "# target floor\nlib 2\nreads 3\nwrites 1\n",
    )
    .unwrap();

    caller.log("suite", &run(0));
    let emptied = caller.core(
        &[
            "test-floor",
            "--targets",
            ".github/test-floors.txt",
            "suite",
        ],
        &[],
    );
    assert_eq!(emptied.status.code(), Some(1), "{}", printed(&emptied));
    assert!(
        String::from_utf8_lossy(&emptied.stdout).contains("writes executed 0 tests, floor is 1"),
        "{}",
        printed(&emptied)
    );
}

/// The agent-core check reads the AGENTS.md of the repository it is run
/// from. Run in place, a check that read this crate's own AGENTS.md would
/// pass every caller, whatever its guide said.
#[test]
fn the_agent_core_check_reads_the_callers_guide_not_this_repositorys() {
    let caller = Caller::new("agents-core");
    fs::write(
        caller.root.join("AGENTS.md"),
        "# A guide with no shared block\n",
    )
    .unwrap();
    let out = caller.core(&["agents-core-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("BEGIN marker"),
        "the refusal does not name the missing marker: {}",
        printed(&out)
    );

    // The caller's guide with the block intact passes.
    fs::copy(repo().join("AGENTS.md"), caller.root.join("AGENTS.md")).unwrap();
    let out = caller.core(&["agents-core-check"], &[]);
    assert!(out.status.success(), "{}", printed(&out));
}
