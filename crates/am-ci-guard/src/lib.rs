//! One check gates a pull request, and it stands for every job.
//!
//! # The rule
//!
//! Branch protection names checks, and a check is a job name. Naming each
//! job means the list has to be edited whenever a job is renamed, split into
//! a matrix leg, or added — and until someone does, the new work is required
//! by nobody. The opposite spelling is worse: a required check no job
//! produces reads as permanently pending, and with `enforce_admins` on
//! nothing merges and there is no failure to point at.
//!
//! So protection names one job — `ci-ok` — which `needs:` every gating job
//! and fails unless each concluded success. This crate is what keeps that
//! true, in every repository at once.
//!
//! # Why a crate and not a copied file
//!
//! `tests/ci_aggregate_gate.rs` existed in eleven repositories as ten
//! distinct files of 161-173 lines. Stripping the module docs, all eleven
//! were the same code: fifty lines of hand-rolled YAML scanning nobody had
//! a reason to write eleven times, and three tests whose assertion text had
//! already drifted apart. Three of those copies were made on one day by
//! three different agents, each told to port it from a sibling.
//!
//! The fix is not a twelfth copy plus a "these must match" test.
//! `antimatter-studios/agent-skills#63`: a declaration with no applier
//! drifts silently, and an in-tree guard cannot detect it because it
//! compares tree files to tree files. Only a single source that cannot be
//! locally edited actually holds — so the rules live here, fetched at run
//! time, and a consumer's test file is three lines that point this crate at
//! the repository.
//!
//! # Use
//!
//! ```toml
//! [dev-dependencies]
//! am-ci-guard = "0.1"
//! ```
//!
//! ```no_run
//! # use am_ci_guard::AggregateGate;
//! #[test]
//! fn one_check_gates_a_pull_request_and_stands_for_every_job() {
//!     AggregateGate::new(env!("CARGO_MANIFEST_DIR")).verify();
//! }
//! ```
//!
//! `env!` is the caller's, expanded in the caller's crate, so it names the
//! consumer's repository rather than this one. It is a compile-time
//! constant, which `std::env::var("CARGO_MANIFEST_DIR")` is not — that is
//! set by cargo when it launches the test binary and is absent when the
//! same binary is run directly, which is how a guard comes to pass by
//! reading nothing.
//!
//! This is a `[dev-dependencies]` entry on purpose. It never enters a
//! consumer's runtime dependency tree, and it is versioned independently of
//! `am-fs-core`, so a driver still pinned to `am-fs-core 0.2.10` by #147 can
//! take it today.
//!
//! # What gates, and what must not
//!
//! Only a workflow that runs on `pull_request` can gate a pull request. A
//! `release.yml` on a `v*.*.*` tag fires after the merge it would be gating
//! has already happened, and a `fuzz.yml` on `workflow_dispatch` plus a
//! nightly cron never sees a pull request at all. Requiring a check from
//! either is requiring a check that never reports, which GitHub reads as
//! permanently pending — a permanent block on every merge, with nothing to
//! point at. So only the gate workflow is scanned, and [`AggregateGate`]
//! refuses if it is not a `pull_request` workflow.
//!
//! Within that workflow, a job carrying a job-level `if:` or
//! `continue-on-error:` is declaring that it does not gate, and both keys
//! break the aggregate if it needs them anyway:
//!
//! - `continue-on-error: true` makes the job's `result` `success` even when
//!   it failed, so the aggregate reads a green tick for a red job. The gate
//!   is on, and it is measuring nothing.
//! - a job-level `if:` that is false on a pull request leaves the job
//!   `skipped`, the aggregate counts `skipped` as not-success, and every
//!   pull request fails on a job that was never meant to run.
//!
//! Either way the job must be declared non-gating and left out of `needs:`.
//! This is `rust-fs-ntfs`'s rule — its `tests/ci_profile.rs` pins
//! `NON_GATING_KEYS = ["if", "continue-on-error"]`, and its `.github-guard`
//! argues the case at length in #158/#278 — generalised so that the
//! declaration is checked against the YAML in both directions. A job you
//! declare non-gating must actually carry one of those keys, and a job that
//! carries one must be declared. Neither list can drift away from the other
//! while this passes.

use std::path::{Path, PathBuf};

use saphyr::{LoadableYamlNode, Yaml};

/// Job-level keys by which a job declares that it does not gate.
///
/// Named here rather than inline because the set is the shared decision,
/// not an implementation detail: `rust-fs-ntfs/tests/ci_profile.rs` pins
/// the same two and its `.github-guard` argues why.
pub const NON_GATING_KEYS: [&str; 2] = ["if", "continue-on-error"];

/// Everything one check of the gate can conclude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The check that failed, named as the test it replaces.
    pub check: &'static str,
    /// What is wrong, and what it costs — written to be read in CI output.
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.check, self.detail)
    }
}

/// The gate's rules, bound to one repository.
///
/// Built with [`AggregateGate::new`] and configured with the builder
/// methods for the differences that are real. Everything else is the same
/// in every repository, which is why it is not configurable: an option
/// nobody needs is an option that lets a repository opt out of the rule.
#[derive(Debug, Clone)]
pub struct AggregateGate {
    repo: PathBuf,
    workflow: String,
    aggregate: String,
    guard_file: String,
    non_gating: Vec<String>,
}

impl AggregateGate {
    /// The gate for the repository rooted at `repo`.
    ///
    /// Pass `env!("CARGO_MANIFEST_DIR")` — see the crate docs for why the
    /// compile-time macro rather than the run-time lookup.
    pub fn new(repo: impl AsRef<Path>) -> Self {
        Self {
            repo: repo.as_ref().to_path_buf(),
            workflow: ".github/workflows/ci.yml".to_string(),
            aggregate: "ci-ok".to_string(),
            guard_file: ".github-guard".to_string(),
            non_gating: Vec::new(),
        }
    }

    /// The workflow that gates, if it is not `.github/workflows/ci.yml`.
    ///
    /// Whatever it is, it has to run on `pull_request`; nothing else can
    /// gate a pull request.
    pub fn workflow(mut self, path: impl Into<String>) -> Self {
        self.workflow = path.into();
        self
    }

    /// The aggregate job's id, if it is not `ci-ok`.
    pub fn aggregate(mut self, job: impl Into<String>) -> Self {
        self.aggregate = job.into();
        self
    }

    /// Jobs that deliberately do not gate, and so are left out of the
    /// aggregate's `needs:`.
    ///
    /// Each one must carry a job-level key from [`NON_GATING_KEYS`] — the
    /// declaration is checked against the workflow, so a job that stops
    /// being conditional and a list that was never updated cannot disagree
    /// in silence. The canonical case is `rust-fs-ntfs`'s `asan`: an
    /// AddressSanitizer build on the nightly toolchain, `continue-on-error:
    /// true` so nightly breakage cannot block a pull request against
    /// stable.
    pub fn non_gating<S: AsRef<str>>(mut self, jobs: impl IntoIterator<Item = S>) -> Self {
        self.non_gating = jobs.into_iter().map(|j| j.as_ref().to_string()).collect();
        self
    }

    /// Run every check, and panic with all of the failures if any failed.
    ///
    /// All of them, not the first: a repository being brought onto the gate
    /// wants the whole list in one run rather than one per push.
    ///
    /// # Panics
    ///
    /// If any check fails, or if a file the gate reads is missing or
    /// unparseable. A guard that cannot read its input has not passed.
    pub fn verify(&self) {
        let failures = self.check();
        assert!(
            failures.is_empty(),
            "the CI gate is not holding in {}:\n\n{}\n\nEach of these is a way for a \
             job to stop gating a merge with nothing failing. See the am-ci-guard \
             crate docs for what each one costs.",
            self.repo.display(),
            failures
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Run every check and return what failed, rather than panicking.
    ///
    /// [`verify`](Self::verify) is the one to call from a test. This is for
    /// the crate's own tests, which have to prove the checks FAIL when they
    /// should — a guard is only demonstrated by its failures.
    pub fn check(&self) -> Vec<Failure> {
        let text = match std::fs::read_to_string(self.repo.join(&self.workflow)) {
            Ok(t) => t,
            Err(e) => {
                return vec![Failure {
                    check: "the_gate_workflow_exists",
                    detail: format!(
                        "cannot read {}: {e}. The gate is this workflow; without it \
                         there is nothing requiring anything.",
                        self.workflow
                    ),
                }]
            }
        };
        let doc = match Yaml::load_from_str(&text) {
            Ok(docs) => match docs.into_iter().next() {
                Some(d) => d,
                None => {
                    return vec![Failure {
                        check: "the_gate_workflow_exists",
                        detail: format!("{} is empty", self.workflow),
                    }]
                }
            },
            Err(e) => {
                return vec![Failure {
                    check: "the_gate_workflow_exists",
                    detail: format!(
                        "{} is not valid YAML: {e}. This guard reads the workflow \
                         rather than scanning its text, so a file it cannot parse is \
                         a failure and never a pass.",
                        self.workflow
                    ),
                }]
            }
        };

        let mut failures = Vec::new();
        failures.extend(self.gate_workflow_runs_on_pull_request(&doc));
        failures.extend(self.aggregate_needs_every_gating_job(&doc));
        failures.extend(self.aggregate_runs_whatever_happened(&doc));
        failures.extend(self.protection_requires_the_aggregate_and_nothing_else());
        failures
    }

    /// Only a `pull_request` workflow can gate a pull request.
    fn gate_workflow_runs_on_pull_request(&self, doc: &Yaml) -> Option<Failure> {
        const CHECK: &str = "the_gate_workflow_runs_on_pull_request";
        // YAML 1.2, so `on:` is the string `on` and not a boolean. A YAML
        // 1.1 parser folds it to `true` and this lookup silently finds
        // nothing -- which is why the parser choice is pinned in Cargo.toml.
        let triggers = match field(doc, "on") {
            Some(t) => t,
            None => {
                return Some(Failure {
                    check: CHECK,
                    detail: format!(
                        "{} has no `on:` trigger at all, so it runs for nothing and \
                         gates nothing",
                        self.workflow
                    ),
                })
            }
        };
        if trigger_names(triggers).iter().any(|t| t == "pull_request") {
            return None;
        }
        Some(Failure {
            check: CHECK,
            detail: format!(
                "{} does not run on `pull_request` (it runs on {:?}), so no check it \
                 produces ever reports on one. Requiring one would be requiring a \
                 check that never arrives, which GitHub reads as permanently pending \
                 -- with enforce_admins on that blocks every merge and there is no \
                 failure to point at.",
                self.workflow,
                trigger_names(triggers)
            ),
        })
    }

    /// Every job that gates is in the aggregate's `needs:`, and every job
    /// that is not, says so.
    fn aggregate_needs_every_gating_job(&self, doc: &Yaml) -> Vec<Failure> {
        const CHECK: &str = "the_aggregate_job_needs_every_other_job";
        let jobs = jobs(doc);
        if !jobs.iter().any(|j| j.id == self.aggregate) {
            return vec![Failure {
                check: CHECK,
                detail: format!(
                    "{} has no `{}` job, so protection has to name every job by hand \
                     -- and that list drifts the moment one is renamed, split or \
                     added. Jobs present: {:?}",
                    self.workflow,
                    self.aggregate,
                    jobs.iter().map(|j| &j.id).collect::<Vec<_>>()
                ),
            }];
        }
        let needs = jobs
            .iter()
            .find(|j| j.id == self.aggregate)
            .map(|j| j.needs.clone())
            .unwrap_or_default();

        let mut failures = Vec::new();

        // A declared non-gating job that carries neither key: the list and
        // the workflow have drifted apart, and the job is silently exempt.
        for declared in &self.non_gating {
            match jobs.iter().find(|j| &j.id == declared) {
                None => failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{declared}` is declared non-gating but is not a job in {}. \
                         An exemption for a job that does not exist is an exemption \
                         waiting to silently cover a future job of that name.",
                        self.workflow
                    ),
                }),
                Some(job) if job.non_gating_keys.is_empty() => failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{declared}` is declared non-gating but carries none of \
                         {NON_GATING_KEYS:?} in {}. It runs unconditionally and its \
                         failure is a real failure, so exempting it from `{}`'s \
                         `needs:` takes a working gate off a working job.",
                        self.workflow, self.aggregate
                    ),
                }),
                Some(_) => {}
            }
        }

        for job in jobs.iter().filter(|j| j.id != self.aggregate) {
            let declared = self.non_gating.contains(&job.id);
            let in_needs = needs.contains(&job.id);
            match (job.non_gating_keys.is_empty(), declared, in_needs) {
                // Ordinary gating job, in needs. Correct.
                (true, false, true) => {}
                // Ordinary gating job, missing from needs: it gates nothing.
                (true, false, false) => failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{}` does not need `{}`, so that job gates nothing: it can \
                         go red and the merge still goes through. `{}` needs {:?}",
                        self.aggregate, job.id, self.aggregate, needs
                    ),
                }),
                // Non-gating job correctly declared and correctly excluded.
                (false, true, false) => {}
                // Non-gating job in needs: whichever key it carries, this
                // breaks. See the crate docs.
                (false, true, true) => failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{}` needs `{}`, which carries {:?} and is declared \
                         non-gating. `continue-on-error` reports `success` for a \
                         failed job, so the aggregate reads green for red; a job-level \
                         `if:` that is false leaves it `skipped`, which the aggregate \
                         counts as not-success and every pull request fails on a job \
                         that was never meant to run. Take it out of `needs:`.",
                        self.aggregate, job.id, job.non_gating_keys
                    ),
                }),
                // Carries a non-gating key but was never declared.
                (false, false, _) => failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{}` carries {:?} but is not declared non-gating. {} Declare \
                         it with `.non_gating([\"{}\"])` and take it out of `{}`'s \
                         `needs:`, or take the key off the job.",
                        job.id,
                        job.non_gating_keys,
                        if in_needs {
                            format!(
                                "It is in `{}`'s `needs:`, where that key is a hole: \
                                 a `continue-on-error` job reports success however it \
                                 ended, and a job skipped by its `if:` fails the \
                                 aggregate on every pull request.",
                                self.aggregate
                            )
                        } else {
                            "It is already out of `needs:`, so it gates nothing -- \
                             but nothing in the repository says that was intended."
                                .to_string()
                        },
                        job.id,
                        self.aggregate
                    ),
                }),
                // Declared non-gating, carries no key, and is in needs. The
                // per-job arm above already reported the drift; saying it
                // twice helps nobody.
                (true, true, _) => {}
            }
        }

        // `needs:` naming a job that does not exist is a workflow GitHub
        // refuses to run at all -- so the gate never reports, and a
        // required check that never reports blocks every merge.
        for need in &needs {
            if !jobs.iter().any(|j| &j.id == need) {
                failures.push(Failure {
                    check: CHECK,
                    detail: format!(
                        "`{}` needs `{need}`, which is not a job in {}. GitHub \
                         refuses to run a workflow with an unresolvable `needs:`, so \
                         the one required check never reports and nothing merges.",
                        self.aggregate, self.workflow
                    ),
                });
            }
        }

        if needs.is_empty() {
            failures.push(Failure {
                check: CHECK,
                detail: format!(
                    "`{}` needs nothing, so it says every job succeeded while asking \
                     none of them",
                    self.aggregate
                ),
            });
        }
        failures
    }

    /// The aggregate has to run even when what it watches did not.
    fn aggregate_runs_whatever_happened(&self, doc: &Yaml) -> Option<Failure> {
        const CHECK: &str = "the_aggregate_runs_whatever_happened";
        let job = jobs(doc).into_iter().find(|j| j.id == self.aggregate)?;
        // `always()` and `${{ always() }}` are the same expression; GitHub
        // accepts a job-level `if:` with or without the braces. Anything
        // else -- `always() && github.event_name == 'pull_request'`, say --
        // is a condition that can be false, and then the aggregate is
        // skipped along with everything it was watching.
        let condition = job.condition.as_deref().map(str::trim).unwrap_or("");
        let bare = condition
            .strip_prefix("${{")
            .and_then(|c| c.strip_suffix("}}"))
            .unwrap_or(condition)
            .trim();
        if bare == "always()" {
            return None;
        }
        Some(Failure {
            check: CHECK,
            detail: if condition.is_empty() {
                format!(
                    "`{}` does not carry `if: always()`, so a cancelled or skipped \
                     job leaves it skipped too -- and a skipped required check never \
                     reports. A job that was skipped is not a job that passed, and an \
                     aggregate that only runs on success cannot say so.",
                    self.aggregate
                )
            } else {
                format!(
                    "`{}` carries `if: {condition}` rather than `if: always()`. Any \
                     condition that can be false is a condition under which the one \
                     required check does not report, and a required check that does \
                     not report reads as permanently pending.",
                    self.aggregate
                )
            },
        })
    }

    /// `.github-guard` requires the aggregate, and nothing else.
    fn protection_requires_the_aggregate_and_nothing_else(&self) -> Option<Failure> {
        const CHECK: &str = "protection_requires_the_aggregate_and_nothing_else";
        let path = self.repo.join(&self.guard_file);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                return Some(Failure {
                    check: CHECK,
                    detail: format!(
                        "cannot read {}: {e}. Without it the required list is \
                         whatever was last typed into the settings page, and nothing \
                         in the repository knows what it says.",
                        self.guard_file
                    ),
                })
            }
        };
        let required = required_checks(&text);
        if required == [self.aggregate.clone()] {
            return None;
        }
        Some(Failure {
            check: CHECK,
            detail: format!(
                "{} requires {:?}; it should name `{}` alone. A job named here as \
                 well drifts the moment it is renamed, and one named here but not \
                 produced by the gate workflow is a required check that never \
                 reports -- permanently pending, blocking every merge with nothing \
                 to point at.",
                self.guard_file, required, self.aggregate
            ),
        })
    }
}

/// One job in the workflow, reduced to what the gate asks about.
#[derive(Debug, Clone)]
struct Job {
    id: String,
    needs: Vec<String>,
    condition: Option<String>,
    /// Which of [`NON_GATING_KEYS`] this job carries.
    non_gating_keys: Vec<&'static str>,
}

/// The value of `name` in a YAML mapping.
fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// A scalar, or every scalar in a sequence, or every key of a mapping.
///
/// `on: pull_request`, `on: [push, pull_request]` and `on:` with a nested
/// mapping are the three spellings GitHub accepts, and they mean the same
/// thing to this question.
fn trigger_names(node: &Yaml) -> Vec<String> {
    if let Some(s) = node.as_str() {
        return vec![s.to_string()];
    }
    if let Some(seq) = node.as_sequence() {
        return seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    if let Some(map) = node.as_mapping() {
        return map
            .iter()
            .filter_map(|(k, _)| k.as_str().map(str::to_string))
            .collect();
    }
    Vec::new()
}

/// A string, or every string in a sequence. `needs: a`, `needs: [a, b]` and
/// a block list all arrive here.
fn string_list(node: &Yaml) -> Vec<String> {
    if let Some(s) = node.as_str() {
        return vec![s.to_string()];
    }
    node.as_sequence()
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every job in the workflow.
fn jobs(doc: &Yaml) -> Vec<Job> {
    let Some(jobs) = field(doc, "jobs").and_then(|j| j.as_mapping()) else {
        return Vec::new();
    };
    jobs.iter()
        .filter_map(|(id, body)| {
            let id = id.as_str()?.to_string();
            let needs = field(body, "needs").map(string_list).unwrap_or_default();
            let condition = field(body, "if").and_then(scalar_text);
            let non_gating_keys = NON_GATING_KEYS
                .iter()
                .copied()
                .filter(|k| field(body, k).is_some())
                .collect();
            Some(Job {
                id,
                needs,
                condition,
                non_gating_keys,
            })
        })
        .collect()
}

/// The text of a scalar, whether it parsed as a string or not.
///
/// `if: always()` is a string, but a bare `if: true` is a boolean and
/// `as_str()` returns `None` for it — and a job whose `if:` is a literal is
/// exactly the case this guard must not read as absent.
fn scalar_text(node: &Yaml) -> Option<String> {
    if let Some(s) = node.as_str() {
        return Some(s.to_string());
    }
    node.as_bool()
        .map(|b| b.to_string())
        .or_else(|| node.as_integer().map(|i| i.to_string()))
}

/// The checks `.github-guard` declares as required.
///
/// `.github-guard` is git-config format, so `git config -f .github-guard
/// --get-all checks.required` is the reference reading. This is that
/// reading, and the part that matters is what it does NOT count: a comment.
/// Every one of these files opens with a long argued rationale, and a
/// scanner that takes any line containing `required =` reads a sentence
/// about what used to be required as a thing that is required.
fn required_checks(text: &str) -> Vec<String> {
    let mut section = String::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if section == "checks" && key.trim().eq_ignore_ascii_case("required") {
            // git-config strips an inline comment from an unquoted value,
            // and the surrounding quotes from a quoted one.
            let value = value.trim();
            let value = if let Some(q) = value.strip_prefix('"') {
                q.split('"').next().unwrap_or("").to_string()
            } else {
                value
                    .split(['#', ';'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            if !value.is_empty() {
                out.push(value);
            }
        }
    }
    out
}
