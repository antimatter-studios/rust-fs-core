//! Every instrumented run of the suite links dead code (#443).
//!
//! Without `-C link-dead-code`, llvm-cov warns that functions have mismatched
//! data and leaves them out of the report. Two things cause it, both seen on
//! macOS at an optimisation level above 0 (CI run 45806719241):
//!
//! * a small public function nothing in the library calls is inferred
//!   cross-crate inlinable. The library records it as unused, under hash 0,
//!   while the test binary that calls it records its profile under a real
//!   hash.
//! * a `#[no_mangle]` function has one name but a different hash in the
//!   unit-test build and in the library build. The macOS linker strips it,
//!   and its profile record with it, from every binary that never calls it.
//!
//! With dead code linked, the library compiles every function it defines and
//! every binary keeps the profile record of every function it carries.

use saphyr::{LoadableYamlNode, Yaml};
use std::path::PathBuf;

const FLAG: &str = "link-dead-code";

fn load(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// True for a line that builds and runs the suite under llvm-cov. `clean` and
/// `report` build nothing.
fn runs_the_suite_instrumented(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with('#') {
        return false;
    }
    line.split("cargo llvm-cov")
        .skip(1)
        .any(|rest| !matches!(rest.split_whitespace().next(), Some("clean" | "report")))
}

#[test]
fn every_instrumented_run_in_ci_links_dead_code() {
    let text = load(".github/workflows/ci.yml");
    let documents = Yaml::load_from_str(&text).expect("ci.yml is YAML");
    let jobs = field(&documents[0], "jobs")
        .and_then(Yaml::as_mapping)
        .expect("ci.yml has jobs");
    let mut runs = 0;
    for (job, body) in jobs {
        let Some(steps) = field(body, "steps").and_then(Yaml::as_sequence) else {
            continue;
        };
        for step in steps {
            let Some(script) = field(step, "run").and_then(Yaml::as_str) else {
                continue;
            };
            let flags = field(step, "env")
                .and_then(|env| field(env, "RUSTFLAGS"))
                .and_then(Yaml::as_str)
                .unwrap_or("");
            for line in script.lines().filter(|l| runs_the_suite_instrumented(l)) {
                runs += 1;
                assert!(
                    flags.contains(FLAG) || line.contains(FLAG),
                    "job {:?} runs the suite under llvm-cov without -C {FLAG}: {line}",
                    job.as_str()
                );
            }
        }
    }
    assert!(runs > 0, "ci.yml no longer runs the suite under llvm-cov");
}

#[test]
fn the_coverage_task_links_dead_code() {
    let text = load("chores.yml");
    let runs: Vec<&str> = text
        .lines()
        .filter(|line| runs_the_suite_instrumented(line))
        .collect();
    assert!(
        !runs.is_empty(),
        "chores.yml no longer runs the suite under llvm-cov"
    );
    for line in runs {
        assert!(
            line.contains(FLAG),
            "chores.yml runs the suite under llvm-cov without -C {FLAG}: {line}"
        );
    }
}

#[test]
fn only_a_run_of_the_suite_counts() {
    assert!(runs_the_suite_instrumented(
        "bash scripts/tier.sh coverage coverage -- cargo llvm-cov --features cli"
    ));
    assert!(!runs_the_suite_instrumented(
        "cargo llvm-cov clean --workspace"
    ));
    assert!(!runs_the_suite_instrumented(
        "cargo llvm-cov report --summary-only"
    ));
    assert!(!runs_the_suite_instrumented(
        "# cargo llvm-cov --features cli"
    ));
}
