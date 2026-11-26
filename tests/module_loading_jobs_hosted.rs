//! A job that loads kernel modules never lands on a runner that cannot (#471).
//!
//! The self-hosted VPS runners have no `modprobe`, and a container runner
//! generally cannot load a module on its host at all. Run 44712622276 failed
//! there with `sudo: modprobe: command not found` and dropped a stack out of the
//! merge queue. So such a job, on Linux, names GitHub's `ubuntu-24.04`, or takes
//! its runner from a gate that asks for the `linux-x64-kvm` class, which never
//! answers a VPS runner: only a KVM runner, or GitHub's `ubuntu-24.04`.
//!
//! The workflows are parsed rather than searched, so a job is judged by its own
//! `runs-on:` and its own steps, not by a comment that names a command.

use saphyr::{LoadableYamlNode, Yaml};
use std::path::PathBuf;

/// The workflows that can run a job which loads kernel modules.
const WORKFLOWS: &[&str] = &["ci.yml", "release.yml"];

/// The runner a module-loading job may name itself.
const HOSTED: &str = "ubuntu-24.04";

/// The gate class such a job may ask for.
const KVM_CLASS: &str = "linux-x64-kvm";

fn workflow(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The value of `name` in a YAML mapping, or `None`.
fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// Every `run:` script in a job, joined into one string.
fn scripts(job: &Yaml) -> String {
    field(job, "steps")
        .and_then(|steps| steps.as_sequence())
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| field(step, "run").and_then(|run| run.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn loads_modules(job: &Yaml) -> bool {
    scripts(job)
        .split_whitespace()
        .any(|word| matches!(word, "modprobe" | "insmod"))
}

/// The class a gate job picks, from its `runner-gate pick` step.
fn gate_class(job: &Yaml) -> Option<String> {
    scripts(job).lines().find_map(|line| {
        line.trim()
            .strip_prefix("runner-gate pick ")
            .map(|class| class.trim().to_string())
    })
}

/// The module-loading jobs of one workflow that can run on Linux, each with
/// whether the rule allows where it runs, and what to say if not.
fn judged(text: &str) -> Vec<(String, bool, String)> {
    let docs = Yaml::load_from_str(text).expect("the workflow parses");
    let jobs: Vec<(&Yaml, &Yaml)> = field(&docs[0], "jobs")
        .and_then(|jobs| jobs.as_mapping())
        .expect("a jobs mapping")
        .iter()
        .collect();
    let mut found = Vec::new();
    for (name, job) in jobs.iter().filter(|(_, job)| loads_modules(job)) {
        let name = name.as_str().unwrap_or("?").to_string();
        let runs_on = field(job, "runs-on")
            .and_then(|runs_on| runs_on.as_str())
            .unwrap_or("<no plain runs-on>");
        // A gated runner is judged by the class its gate asks for.
        let class = runs_on
            .split("needs.")
            .nth(1)
            .and_then(|rest| rest.split(".outputs").next())
            .and_then(|gate| jobs.iter().find(|(n, _)| n.as_str() == Some(gate)))
            .and_then(|(_, gate)| gate_class(gate));
        // A macOS or Windows leg carries the Linux-only step and never runs it.
        if class
            .as_deref()
            .is_some_and(|class| !class.starts_with("linux"))
        {
            continue;
        }
        let allowed = runs_on == HOSTED || class.as_deref() == Some(KVM_CLASS);
        found.push((
            name,
            allowed,
            format!("runs-on `{runs_on}`, gate class {class:?}"),
        ));
    }
    found
}

#[test]
fn every_module_loading_job_runs_where_modules_can_be_loaded() {
    let mut checked = 0;
    let mut offending = Vec::new();
    for file in WORKFLOWS {
        for (job, allowed, why) in judged(&workflow(file)) {
            checked += 1;
            if !allowed {
                offending.push(format!("{file}: job `{job}` {why}"));
            }
        }
    }
    // Fewer would mean the detection stopped finding the jobs, and the loop
    // above would pass having checked nothing: ci.yml's Linux test and
    // coverage jobs, and release.yml's test job.
    assert!(
        checked >= 3,
        "expected at least 3 module-loading Linux jobs, found {checked}"
    );
    assert!(
        offending.is_empty(),
        "a job that loads kernel modules names `{HOSTED}` or asks the gate for \
         `{KVM_CLASS}` (#471):\n{}",
        offending.join("\n")
    );
}

/// The rule can fail: each way of reaching a runner, judged.
#[test]
fn the_rule_refuses_the_vps_class_and_allows_the_kvm_class() {
    let job = |class: &str| {
        format!(
            "jobs:\n  gate:\n    runs-on: antimatter-vps-gate\n    steps:\n      \
             - run: runner-gate pick {class}\n  work:\n    \
             runs-on: ${{{{ needs.gate.outputs.runner }}}}\n    steps:\n      \
             - run: sudo modprobe zfs\n"
        )
    };
    let allowed = |text: &str| judged(text).into_iter().map(|j| j.1).collect::<Vec<_>>();
    assert_eq!(allowed(&job("linux-x64")), [false]);
    assert_eq!(allowed(&job("linux-x64-kvm")), [true]);
    assert_eq!(allowed(&job("macos-arm64")), Vec::<bool>::new());
    let literal = |runner: &str| {
        format!("jobs:\n  work:\n    runs-on: {runner}\n    steps:\n      - run: insmod x.ko\n")
    };
    assert_eq!(allowed(&literal("ubuntu-24.04")), [true]);
    assert_eq!(allowed(&literal("antimatter-vps-linux-x64")), [false]);
}
