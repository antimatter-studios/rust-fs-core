//! `scripts/branch-reap.sh`, which `.github/workflows/branch-reap.yml` runs
//! when a pull request merges: it deletes the pull request's branch, unless
//! something still needs it.
//!
//! The branch lives in a real bare repository under this one's `tmp/`, so
//! the delete is git's own `--force-with-lease`, the same push the workflow
//! makes. `gh` is a stub that answers the questions the script asks: the
//! default branch, whether a branch is protected, and which open pull
//! requests have it as their base or head. Every refusal is proved on its
//! own: each one is a branch somebody still needs.
//!
//! UNIX ONLY, AND NOT A SKIP: the workflow runs on ubuntu-latest.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const REPO: &str = "example-org/example";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.org")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.org")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", printed(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn printed(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A bare "remote" holding `main` and the branches a case names, a work
/// tree to make commits in, and the stub's answers. Removed on drop.
struct Remote {
    root: PathBuf,
}

impl Remote {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = repo().join("tmp").join(format!(
            "branch-reap-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let remote = Remote { root };
        fs::create_dir_all(remote.answers()).unwrap();
        let bare = remote.bare();
        let work = remote.root.join("work");
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .status()
            .unwrap();
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&work)
            .status()
            .unwrap();
        git(&work, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main"]);
        fs::write(remote.answers().join("default"), "main\n").unwrap();
        let stub = remote.root.join("gh");
        fs::write(&stub, STUB).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        remote
    }

    fn bare(&self) -> PathBuf {
        self.root.join("remote.git")
    }

    fn answers(&self) -> PathBuf {
        self.root.join("answers")
    }

    /// Push a new commit as `branch`, and return it.
    fn branch(&self, branch: &str) -> String {
        let work = self.root.join("work");
        git(&work, &["checkout", "-q", "-B", branch, "main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", branch]);
        git(
            &work,
            &["push", "-q", "-f", self.bare().to_str().unwrap(), branch],
        );
        git(&work, &["rev-parse", "HEAD"])
    }

    /// What the stub answers for `kind` (`protected`, `base`, `head`) about
    /// `branch`.
    fn answer(&self, kind: &str, branch: &str, text: &str) {
        let name = format!("{kind}.{}", branch.replace('/', "_"));
        fs::write(self.answers().join(name), text).unwrap();
    }

    fn has(&self, branch: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(self.bare())
            .args([
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap()
            .status
            .success()
    }

    fn reap(&self, branch: &str, sha: &str) -> Output {
        Command::new("bash")
            .arg(repo().join("scripts").join("branch-reap.sh"))
            .args([REPO, branch, sha, "7"])
            .env("BRANCH_REAP_GH", self.root.join("gh"))
            .env("BRANCH_REAP_REMOTE", self.bare())
            .env("STUB_ANSWERS", self.answers())
            .output()
            .expect("branch-reap.sh runs")
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `gh api repos/R --jq .default_branch`, `gh api repos/R/branches/B --jq
/// .protected` (absent answer: not protected; `missing`: no such branch),
/// and `gh pr list ... --base B` / `--head B` (absent answer: none).
const STUB: &str = r#"#!/usr/bin/env bash
A="$STUB_ANSWERS"
enc() { printf '%s' "$1" | tr '/' '_'; }
if [ "$1" = api ]; then
    case "$2" in
        */branches/*)
            b="${2#*/branches/}"
            f="$A/protected.$(enc "$b")"
            if [ -f "$f" ]; then
                [ "$(cat "$f")" = missing ] && exit 1
                cat "$f"
            else
                echo false
            fi
            exit 0 ;;
        *) cat "$A/default"; exit 0 ;;
    esac
fi
if [ "$1" = pr ] && [ "$2" = list ]; then
    for ((i = 1; i <= $#; i++)); do
        case "${!i}" in
            --base) j=$((i + 1)); f="$A/base.$(enc "${!j}")" ;;
            --head) j=$((i + 1)); f="$A/head.$(enc "${!j}")" ;;
        esac
    done
    [ -f "$f" ] && cat "$f"
    exit 0
fi
echo "stub gh: unexpected $*" >&2
exit 3
"#;

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_merged_branch_nothing_needs_is_deleted() {
    let r = Remote::new("done");
    let sha = r.branch("feat/done");
    let out = r.reap("feat/done", &sha);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        stdout(&out).contains(&format!("deleted feat/done (#7) at {sha}")),
        "{}",
        printed(&out)
    );
    assert!(!r.has("feat/done"), "the branch is still there");
    assert!(r.has("main"), "main went with it");
}

#[test]
fn a_branch_pushed_to_after_its_merge_is_kept() {
    let r = Remote::new("moved");
    let merged = r.branch("feat/moved");
    let work = r.root.join("work");
    git(
        &work,
        &["commit", "-q", "--allow-empty", "-m", "after the merge"],
    );
    git(
        &work,
        &["push", "-q", r.bare().to_str().unwrap(), "feat/moved"],
    );
    let out = r.reap("feat/moved", &merged);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        stdout(&out).contains("it moved after the merge"),
        "{}",
        printed(&out)
    );
    assert!(r.has("feat/moved"), "a commit nobody merged was deleted");
}

#[test]
fn a_branch_a_stacked_pull_request_is_based_on_is_kept() {
    let r = Remote::new("base");
    let sha = r.branch("feat/base");
    r.answer("base", "feat/base", "#12 #13");
    let out = r.reap("feat/base", &sha);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        stdout(&out).contains("open pull requests are based on it: #12 #13"),
        "{}",
        printed(&out)
    );
    assert!(
        r.has("feat/base"),
        "a stacked pull request's base was deleted"
    );
}

#[test]
fn a_branch_reused_by_an_open_pull_request_is_kept() {
    let r = Remote::new("head");
    let sha = r.branch("feat/again");
    r.answer("head", "feat/again", "#20");
    let out = r.reap("feat/again", &sha);
    assert!(
        stdout(&out).contains("it is the head of open pull requests: #20"),
        "{}",
        printed(&out)
    );
    assert!(r.has("feat/again"));
}

#[test]
fn the_default_protected_queue_and_wip_branches_are_kept() {
    let r = Remote::new("kept");
    let main = git(&r.root.join("work"), &["rev-parse", "main"]);
    let out = r.reap("main", &main);
    assert!(
        stdout(&out).contains("it is the default branch"),
        "{}",
        printed(&out)
    );
    assert!(r.has("main"));

    let sha = r.branch("release/1.x");
    r.answer("protected", "release/1.x", "true");
    let out = r.reap("release/1.x", &sha);
    assert!(
        stdout(&out).contains("it is protected"),
        "{}",
        printed(&out)
    );
    assert!(r.has("release/1.x"));

    for branch in ["gh-readonly-queue/main/pr-7-abc", "wip/rescued"] {
        let sha = r.branch(branch);
        let out = r.reap(branch, &sha);
        assert!(stdout(&out).contains("kept"), "{}", printed(&out));
        assert!(r.has(branch), "{branch} was deleted");
    }
}

#[test]
fn a_branch_already_gone_is_not_an_error() {
    let r = Remote::new("gone");
    r.answer("protected", "feat/gone", "missing");
    let out = r.reap("feat/gone", "0123456789012345678901234567890123456789");
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        stdout(&out).contains("it is already gone"),
        "{}",
        printed(&out)
    );
}

/// The workflow is what calls the script, and nothing else here runs it, so
/// its own properties are read from the file.
#[test]
fn the_workflow_reaps_only_a_merged_pull_request_from_this_repository() {
    let text = fs::read_to_string(repo().join(".github/workflows/branch-reap.yml"))
        .expect("the reusable workflow exists");
    for needle in [
        "workflow_call:",
        "github.event.pull_request.merged == true",
        "github.event.pull_request.head.repo.full_name == github.repository",
        "contents: write",
        "scripts/branch-reap.sh",
        // The branch name and commit reach the script through env, never
        // through ${{ }} inside a run: block, where a crafted branch name
        // would be shell.
        "BRANCH: ${{ github.event.pull_request.head.ref }}",
        "MERGED: ${{ github.event.pull_request.head.sha }}",
        "\"$BRANCH\"",
    ] {
        assert!(text.contains(needle), "branch-reap.yml lacks {needle:?}");
    }
    // Every line of every run: step, a block scalar's lines included.
    let mut run_indent: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(at) = run_indent {
            if line.trim().is_empty() || indent > at {
                assert!(
                    !line.contains("${{"),
                    "a run: block interpolates an expression: {line}"
                );
                continue;
            }
            run_indent = None;
        }
        let code = line.trim_start().trim_start_matches("- ");
        if code.starts_with("run:") {
            assert!(
                !line.contains("${{"),
                "a run: line interpolates an expression: {line}"
            );
            run_indent = Some(indent);
        }
    }
}
