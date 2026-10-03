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

/// A throwaway repository under this one's `tmp/`, removed on drop. It holds
/// a copy of `scripts/core.sh`, as every consumer does, and a log directory.
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
        fs::copy(
            repo().join("scripts").join("core.sh"),
            caller.root.join("scripts").join("core.sh"),
        )
        .expect("rust-fs-core ships scripts/core.sh for its consumers");
        fs::write(
            caller.root.join("Cargo.toml"),
            "[package]\nname = \"am-example\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
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

    /// `bash scripts/core.sh ARGS...` from the caller, finding this crate
    /// through FS_CORE_ROOT the way a CI job with a checkout does.
    fn core(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(bash());
        command
            .current_dir(&self.root)
            .arg("scripts/core.sh")
            .args(args)
            .env("FS_CORE_ROOT", repo());
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
    assert_eq!(version_of("ci-gate.sh"), "rust-fs-core-ci-gate 1");
    assert_eq!(version_of("package-cli.sh"), "rust-fs-core-package-cli 1");
}

/// `ci-gate` is served the way the others are: by name, through the
/// bootstrap, with its contract checked before it runs. Its behaviour against
/// a caller's `ci.yml` and `.github-guard` is `tests/scripts/test-ci-gate.sh`,
/// which needs python3's yaml module and so runs where CI provides one.
#[test]
fn the_bootstrap_serves_ci_gate() {
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
        !said.contains("am-fs-core"),
        "it checked this crate, not the caller's: {}",
        printed(&out)
    );
}

#[test]
fn the_bootstrap_refuses_what_it_cannot_vouch_for() {
    let caller = Caller::new("refuse");
    let unknown = caller.core(&["no-such-script"], &[]);
    assert_eq!(unknown.status.code(), Some(2), "{}", printed(&unknown));

    // A core that is not there.
    let missing = Command::new(bash())
        .current_dir(&caller.root)
        .args(["scripts/core.sh", "test-floor", "unit", "1"])
        .env("FS_CORE_ROOT", caller.root.join("no-core-here"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1), "{}", printed(&missing));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("test-floor.sh"),
        "{}",
        printed(&missing)
    );

    // A core whose script does not answer --version as the bootstrap asks.
    let fake = caller.root.join("fake-core");
    fs::create_dir_all(fake.join("scripts")).unwrap();
    fs::write(fake.join("scripts/test-floor.sh"), "echo something-else\n").unwrap();
    let wrong = Command::new(bash())
        .current_dir(&caller.root)
        .args(["scripts/core.sh", "test-floor", "unit", "1"])
        .env("FS_CORE_ROOT", &fake)
        .output()
        .unwrap();
    assert_eq!(wrong.status.code(), Some(1), "{}", printed(&wrong));
    assert!(
        String::from_utf8_lossy(&wrong.stderr).contains("--version"),
        "{}",
        printed(&wrong)
    );
}

#[test]
fn this_repository_runs_its_own_scripts_through_the_same_bootstrap() {
    // No FS_CORE_ROOT, no sibling: cargo names this crate as am-fs-core.
    let out = Command::new(bash())
        .current_dir(repo())
        .args(["scripts/core.sh", "test-floor", "--version"])
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
    ] {
        assert!(
            Path::new(&repo().join("scripts").join(script)).is_file(),
            "scripts/{script}"
        );
    }
}

#[test]
fn family_check_passes_a_clean_caller_and_refuses_each_kind_of_copy() {
    let clean = Caller::new("family-clean");
    fs::write(
        clean.root.join("chores.yml"),
        "tasks:\n  t:\n    cmds:\n      - 'bash scripts/core.sh test-floor debug 1'\n",
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
        String::from_utf8_lossy(&out.stdout).contains("scripts/test-floor.sh is a copy"),
        "{}",
        printed(&out)
    );

    // The guest toolchain install is a family script like the others: five
    // repositories carried a copy of it once, and only one recovered from an
    // interrupted install. A copy, or a call that bypasses core.sh, is refused.
    let guest = Caller::new("family-guest-copy");
    fs::write(
        guest.root.join("scripts/guest-rust-toolchain.sh"),
        "echo mine\n",
    )
    .unwrap();
    fs::write(
        guest.root.join("scripts/guest-suite.sh"),
        "bash scripts/guest-rust-toolchain.sh\n",
    )
    .unwrap();
    let out = guest.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("scripts/guest-rust-toolchain.sh is a copy"),
        "{}",
        printed(&out)
    );
    assert!(
        said.contains("scripts/guest-suite.sh:1:bash scripts/guest-rust-toolchain.sh"),
        "{}",
        printed(&out)
    );

    // A committed copy of ci-gate, the one every repository used to carry.
    let gate = Caller::new("family-gate-copy");
    fs::write(gate.root.join("scripts/ci-gate.sh"), "echo mine\n").unwrap();
    let out = gate.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("scripts/ci-gate.sh is a copy of rust-fs-core's; delete it and run it as scripts/core.sh ci-gate"),
        "{}",
        printed(&out)
    );

    // A bootstrap edited away from core's.
    let edited = Caller::new("family-edited");
    let path = edited.root.join("scripts/core.sh");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("# local tweak\n");
    fs::write(&path, text).unwrap();
    let out = edited.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("differs from rust-fs-core's"),
        "{}",
        printed(&out)
    );

    // A workflow still calling a local copy -- and a comment naming one is fine.
    let calls = Caller::new("family-calls");
    fs::create_dir_all(calls.root.join(".github/workflows")).unwrap();
    fs::write(
        calls.root.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      # see scripts/semver-check.sh\n      - run: bash scripts/test-floor.sh debug 1\n",
    )
    .unwrap();
    let out = calls.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("bash scripts/test-floor.sh debug 1"),
        "{}",
        printed(&out)
    );
    assert!(
        !said.contains("see scripts/semver-check.sh"),
        "a comment was counted as a call: {}",
        printed(&out)
    );

    // chores.yml still naming the local ci-gate, as eleven of them did.
    let gate_call = Caller::new("family-gate-call");
    fs::write(
        gate_call.root.join("chores.yml"),
        "tasks:\n  check:ci-gate:\n    cmds:\n      - scripts/ci-gate.sh\n",
    )
    .unwrap();
    let out = gate_call.core(&["family-check"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("- scripts/ci-gate.sh"),
        "{}",
        printed(&out)
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
