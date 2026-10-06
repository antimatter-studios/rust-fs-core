//! A release's notes are its CHANGELOG section, and a draft of that section
//! can be made from the commits since the last tag (#209).
//!
//! The family's GitHub releases said only "See CHANGELOG.md" while the
//! CHANGELOG held the useful text: what broke and why, what was added and
//! fixed. Two family scripts close that gap, run through `scripts/core.sh`
//! from a caller repository as every other family script is:
//!
//! - `release-notes VERSION` prints the `## [VERSION]` section, a link to the
//!   diff from the previous version, and the provenance line, and fails when
//!   the CHANGELOG has no section for the version: a release without notes is
//!   refused rather than published blank.
//! - `changelog-draft` turns the squash commits since the newest tag into
//!   Added / Fixed / Changed bullets, each linking its pull request, printed
//!   or (`--write`) put under `## [Unreleased]`. It is a starting point; the
//!   explanation of a breaking or visible change is still written by hand.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Git's bash on Windows: a bare `bash` there is WSL's launcher, which has no
/// distribution on the runner (the same choice tests/family_scripts.rs makes).
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

/// A throwaway caller repository under this one's `tmp/`, removed on drop,
/// holding the `scripts/core.sh` bootstrap every consumer carries.
struct Caller {
    root: PathBuf,
}

impl Caller {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = repo().join("tmp").join(format!(
            "release-notes-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let caller = Caller { root };
        fs::create_dir_all(&caller.root).unwrap();
        // A repository of its own: a script run in place works on the
        // repository it is run from, and this tree is inside this crate's.
        let out = Command::new("git")
            .current_dir(&caller.root)
            .args(["init", "-q", "-b", "main"])
            .output()
            .unwrap();
        assert!(out.status.success(), "git init: {}", printed(&out));
        caller
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// The script run in place from this crate, from the caller's directory.
    fn core(&self, args: &[&str]) -> Output {
        let (name, rest) = args.split_first().expect("a script name");
        Command::new(bash())
            .current_dir(&self.root)
            .arg(repo().join("scripts").join(format!("{name}.sh")))
            .args(rest)
            .env_remove("FS_CORE_CALLER")
            .env("GITHUB_REPOSITORY", "example-org/rust-example")
            .output()
            .unwrap()
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(&self.root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", printed(&out));
    }

    fn commit(&self, subject: &str) {
        self.git(&[
            "commit",
            "-q",
            "--allow-empty",
            "--no-verify",
            "-m",
            subject,
        ]);
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const CHANGELOG: &str = "# Changelog

## [Unreleased]

## [0.2.0] — 2026-10-06

### Breaking

- **`Error` gains a variant.** A `match` needs one more arm.

### Added

- A thing that was missing (#12).

## [0.1.0] — 2026-09-01

### Added

- The first release.
";

fn script_version(script: &str) -> String {
    let out = Command::new(bash())
        .arg(repo().join("scripts").join(script))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn both_scripts_state_their_contract() {
    assert_eq!(
        script_version("release-notes.sh"),
        "rust-fs-core-release-notes 1"
    );
    assert_eq!(
        script_version("changelog-draft.sh"),
        "rust-fs-core-changelog-draft 1"
    );
}

#[test]
fn the_notes_are_the_versions_section_with_its_diff_and_provenance() {
    let caller = Caller::new("section");
    caller.write("CHANGELOG.md", CHANGELOG);
    let out = caller.core(&["release-notes", "0.2.0"]);
    assert!(out.status.success(), "{}", printed(&out));
    let notes = String::from_utf8_lossy(&out.stdout);
    assert!(
        notes.contains("### Breaking") && notes.contains("A `match` needs one more arm."),
        "the version's own text is missing:\n{notes}"
    );
    assert!(
        notes.contains("A thing that was missing (#12)."),
        "the Added list is missing:\n{notes}"
    );
    assert!(
        !notes.contains("The first release."),
        "another version's section leaked in:\n{notes}"
    );
    assert!(
        !notes.contains("## [0.2.0]"),
        "the heading repeats the release title:\n{notes}"
    );
    assert!(
        notes.contains("https://github.com/example-org/rust-example/compare/v0.1.0...v0.2.0"),
        "no link to the diff from the previous version:\n{notes}"
    );
    assert!(
        notes.contains("attestation"),
        "the provenance line is missing:\n{notes}"
    );
}

#[test]
fn a_leading_v_in_the_version_is_accepted() {
    let caller = Caller::new("v");
    caller.write("CHANGELOG.md", CHANGELOG);
    let out = caller.core(&["release-notes", "v0.2.0"]);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("### Breaking"));
}

#[test]
fn a_version_the_changelog_does_not_describe_is_refused() {
    let caller = Caller::new("missing");
    caller.write("CHANGELOG.md", CHANGELOG);
    let out = caller.core(&["release-notes", "0.3.0"]);
    assert_eq!(out.status.code(), Some(1), "{}", printed(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("0.3.0"),
        "the refusal does not name the version: {}",
        printed(&out)
    );
    assert!(
        out.stdout.is_empty(),
        "notes were printed anyway: {}",
        printed(&out)
    );
}

#[test]
fn the_draft_groups_the_commits_since_the_newest_tag() {
    let caller = Caller::new("draft");
    caller.write("CHANGELOG.md", CHANGELOG);
    caller.git(&["add", "-A"]);
    caller.commit("chore(release): 0.2.0 (#20)");
    caller.git(&["tag", "v0.2.0"]);
    caller.commit("feat: a volume can be relabelled (#21)");
    caller.commit("fix: a short read is refused (#22)");
    caller.commit("ci: the budget carries the new suite (#23)");
    caller.commit("chore(release): 0.2.1 (#24)");

    let out = caller.core(&["changelog-draft"]);
    assert!(out.status.success(), "{}", printed(&out));
    let draft = String::from_utf8_lossy(&out.stdout);
    let added = draft.find("### Added").expect("no Added group");
    let fixed = draft.find("### Fixed").expect("no Fixed group");
    let changed = draft.find("### Changed").expect("no Changed group");
    assert!(
        added < fixed && fixed < changed,
        "groups out of order:\n{draft}"
    );
    assert!(
        draft.contains(
            "- A volume can be relabelled ([#21](https://github.com/example-org/rust-example/pull/21))."
        ),
        "the feat bullet is not the subject with its pull request linked:\n{draft}"
    );
    assert!(draft.contains("A short read is refused"), "{draft}");
    assert!(
        draft.contains("The budget carries the new suite"),
        "{draft}"
    );
    assert!(
        !draft.contains("0.2.1") && !draft.contains("0.2.0"),
        "a release commit became an entry:\n{draft}"
    );
}

#[test]
fn the_draft_can_be_written_under_unreleased() {
    let caller = Caller::new("write");
    caller.write("CHANGELOG.md", CHANGELOG);
    caller.git(&["add", "-A"]);
    caller.commit("chore(release): 0.2.0 (#20)");
    caller.git(&["tag", "v0.2.0"]);
    caller.commit("fix: a short read is refused (#22)");

    let out = caller.core(&["changelog-draft", "--write"]);
    assert!(out.status.success(), "{}", printed(&out));
    let text = fs::read_to_string(caller.root.join("CHANGELOG.md")).unwrap();
    let unreleased = text.find("## [Unreleased]").unwrap();
    let entry = text
        .find("A short read is refused")
        .expect("draft not written");
    let released = text.find("## [0.2.0]").unwrap();
    assert!(
        unreleased < entry && entry < released,
        "the draft is not under [Unreleased]:\n{text}"
    );
}

/// A CHANGELOG that heads its sections `## vX.Y.Z` or `## X.Y.Z`, rather than
/// Keep a Changelog's `## [X.Y.Z]`, is read the same way: several
/// repositories in the family use the `v` form, and one has a hook that
/// requires it.
#[test]
fn a_section_headed_without_brackets_is_found_too() {
    for (style, heading) in [("v", "v{}"), ("bare", "{}")] {
        let caller = Caller::new(style);
        let changelog = CHANGELOG
            .replace("## [Unreleased]", "## Unreleased")
            .replace(
                "## [0.2.0]",
                &format!("## {}", heading.replace("{}", "0.2.0")),
            )
            .replace(
                "## [0.1.0]",
                &format!("## {}", heading.replace("{}", "0.1.0")),
            );
        caller.write("CHANGELOG.md", &changelog);
        let out = caller.core(&["release-notes", "0.2.0"]);
        assert!(out.status.success(), "{style}: {}", printed(&out));
        let notes = String::from_utf8_lossy(&out.stdout);
        assert!(notes.contains("### Breaking"), "{style}: {notes}");
        assert!(
            !notes.contains("The first release."),
            "{style}: another version's section leaked in:\n{notes}"
        );
        assert!(
            notes.contains("compare/v0.1.0...v0.2.0"),
            "{style}: no link to the diff from the previous version:\n{notes}"
        );
        let missing = caller.core(&["release-notes", "0.3.0"]);
        assert_eq!(
            missing.status.code(),
            Some(1),
            "{style}: {}",
            printed(&missing)
        );
    }
}

/// A repository whose release workflow attaches files without attesting them
/// says so: `--unattested` swaps the provenance line for one that does not
/// claim an attestation the release does not have.
#[test]
fn an_unattested_release_does_not_claim_an_attestation() {
    let caller = Caller::new("unattested");
    caller.write("CHANGELOG.md", CHANGELOG);
    let out = caller.core(&["release-notes", "--unattested", "0.2.0"]);
    assert!(out.status.success(), "{}", printed(&out));
    let notes = String::from_utf8_lossy(&out.stdout);
    assert!(notes.contains("### Breaking"), "{notes}");
    assert!(
        !notes.contains("attestation"),
        "an unattested release claims an attestation:\n{notes}"
    );
    assert!(
        notes.contains("this tag's release workflow built"),
        "the provenance line is missing:\n{notes}"
    );
}

/// The diff link compares with the previous RELEASE: a heading like
/// `## 0.2.0-dev` or `## Before 0.1.4` between two releases is history, not a
/// tag, and a link to it is a link to nothing.
#[test]
fn the_diff_link_skips_a_heading_that_is_not_a_release() {
    let caller = Caller::new("prev");
    caller.write(
        "CHANGELOG.md",
        "# Changelog\n\n## Unreleased\n\n## v0.1.5 — 2026-10-02\n\n- A fix.\n\n\
         ## Before v0.1.4 — merged\n\n- History.\n\n## 0.2.0-dev — pre-merge\n\n- More.\n\n\
         ## 0.1.0 — initial\n\n- The first.\n",
    );
    let out = caller.core(&["release-notes", "0.1.5"]);
    assert!(out.status.success(), "{}", printed(&out));
    let notes = String::from_utf8_lossy(&out.stdout);
    assert!(
        !notes.contains("0.2.0-dev"),
        "the diff link names a heading that is not a release:\n{notes}"
    );
    assert!(
        notes.contains("compare/v0.1.0...v0.1.5"),
        "no link to the previous release:\n{notes}"
    );
}
