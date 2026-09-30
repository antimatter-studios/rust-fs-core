//! `fs_core::cli`, driven the way a tool drives it: a [`Family`] of demo
//! tools, run through [`cli::respond`] with an explicit argument vector, so
//! every assertion is about what would be printed and the exit status,
//! without spawning the binary.
//!
//! `doctor` is the exception: its question is what PATH runs, so its tests
//! put small scripts on a PATH of their own and ask.
//!
//! Built only with the `cli` feature; `chore test:debug` and CI pass it.

#![cfg(feature = "cli")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fs_core::cli::clap::{Arg, ArgMatches, Command};
use fs_core::cli::{self, dispatch, docs, doctor, output, CliError, Family, Json, Outcome, Tool};

// ---------------------------------------------------------------------------
// The demo family
// ---------------------------------------------------------------------------

fn demo_command() -> Command {
    let sub = |name: &'static str| Command::new(name).about(name);
    Command::new("fs")
        .about("Work on a demo image")
        .args(cli::format_args().map(|a| a.global(true)))
        .subcommand_required(true)
        .subcommand(sub("report"))
        .subcommand(sub("custom-text"))
        .subcommand(sub("silent"))
        .subcommand(sub("fail"))
        .subcommand(sub("unsupported"))
        .subcommand(sub("refuse"))
        .subcommand(sub("corrected"))
        .subcommand(sub("raw"))
        .subcommand(sub("hidden").hide(true))
        .after_help("Examples:\n  fs.demo report\n  fs.demo report --text")
}

fn demo_run(matches: &ArgMatches) -> Result<Outcome, CliError> {
    let report = || {
        Json::object([
            ("fs", Json::from("demo")),
            ("label", Json::Null),
            ("demo", Json::object([("blocks", Json::from(8u64))])),
        ])
    };
    match matches.subcommand_name() {
        Some("report") => Ok(Outcome::report(report())),
        Some("custom-text") => Ok(Outcome::report(report()).with_text("a person's summary")),
        Some("silent") => Ok(Outcome::report(report()).with_text("")),
        Some("fail") => Err(CliError::failed("open disk.img: no such file")),
        Some("unsupported") => Err(CliError::not_implemented("resize")),
        Some("refuse") => Err(CliError::refused("the format is read-only")),
        Some("corrected") => Ok(Outcome::report(report()).with_code(1)),
        Some("raw") => Ok(Outcome::done()),
        other => panic!("unexpected subcommand {other:?}"),
    }
}

fn check_command() -> Command {
    Command::new("fsck")
        .about("Check a demo image")
        .args(cli::format_args())
        .arg(Arg::new("image").required(true))
        .after_help("Examples:\n  fsck.demo disk.img")
}

fn check_run(_: &ArgMatches) -> Result<Outcome, CliError> {
    Err(CliError::failed("errors left uncorrected").with_code(4))
}

static FAMILY: Family = Family {
    repo: "rust-fs-demo",
    crate_name: "am-fs-demo",
    version: "1.2.3",
    about: "Demo tools: work on a demo image without mounting it",
    install_hints: &[
        "`chore cli:install` from a checkout",
        "`brew install example/tap/rust-fs-demo`",
    ],
    tools: &[
        Tool {
            name: "fs.demo",
            verb: "fs",
            section: 1,
            about: "Work on a demo image",
            usage_exit: output::EXIT_USAGE,
            command: demo_command,
            run: demo_run,
        },
        Tool {
            name: "fsck.demo",
            verb: "fsck",
            section: 8,
            about: "Check a demo image",
            // fsck(8)'s scheme: 2 there means "reboot".
            usage_exit: 16,
            command: check_command,
            run: check_run,
        },
    ],
};

fn argv(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn respond(args: &[&str]) -> cli::Response {
    cli::respond(&FAMILY, argv(args))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn resolved(args: &[&str]) -> (Option<&'static str>, Vec<String>) {
    let strings = |v: Vec<OsString>| {
        v.into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };
    match dispatch::resolve(&FAMILY, argv(args)) {
        dispatch::Target::Tool(tool, rest) => (Some(tool.name), strings(rest)),
        dispatch::Target::Repo(rest) => (None, strings(rest)),
    }
}

#[test]
fn a_dotted_name_is_that_tool_wherever_it_is_installed() {
    assert_eq!(
        resolved(&["fs.demo", "report"]),
        (Some("fs.demo"), vec!["fs.demo".into(), "report".into()])
    );
    assert_eq!(
        resolved(&["/opt/bin/fsck.demo", "x.img"]).0,
        Some("fsck.demo")
    );
    assert_eq!(
        resolved(&["C:/tools/fs.demo.exe", "report"]).0,
        Some("fs.demo")
    );
}

#[test]
fn any_other_name_reaches_a_tool_by_verb_or_by_full_name() {
    for word in ["fs", "fs.demo"] {
        assert_eq!(
            resolved(&["/usr/local/bin/rust-fs-demo", word, "report"]),
            (Some("fs.demo"), vec!["fs.demo".into(), "report".into()]),
            "{word}"
        );
    }
    // A renamed copy, and cargo's own test binary, are the entry point too.
    assert_eq!(
        resolved(&["renamed-by-a-packager", "fsck", "x"]).0,
        Some("fsck.demo")
    );
}

#[test]
fn an_unknown_name_is_the_entry_point_and_keeps_its_arguments() {
    assert_eq!(
        resolved(&["rust-fs-demo", "doctor"]),
        (None, vec!["rust-fs-demo".into(), "doctor".into()])
    );
    assert_eq!(
        resolved(&["rust-fs-demo", "mkfs"]),
        (None, vec!["rust-fs-demo".into(), "mkfs".into()])
    );
    // No argv[0] at all: the entry point, named for the repository.
    assert_eq!(resolved(&[]), (None, vec!["rust-fs-demo".into()]));
}

#[test]
fn the_invoked_name_drops_the_directory_and_a_windows_suffix() {
    assert_eq!(dispatch::invoked_name(Path::new("/a/b/fs.demo")), "fs.demo");
    assert_eq!(dispatch::invoked_name(Path::new("fs.demo.exe")), "fs.demo");
    assert_eq!(dispatch::invoked_name(Path::new("/")), "");
}

#[test]
fn a_family_finds_tools_by_name_and_by_word() {
    assert_eq!(FAMILY.by_name("fsck.demo").map(|t| t.verb), Some("fsck"));
    assert!(
        FAMILY.by_name("fsck").is_none(),
        "a verb is not an installed name"
    );
    assert_eq!(FAMILY.by_word("fsck").map(|t| t.name), Some("fsck.demo"));
    assert_eq!(
        FAMILY.by_word("fsck.demo").map(|t| t.name),
        Some("fsck.demo")
    );
    assert!(FAMILY.by_word("mkfs").is_none());
}

// ---------------------------------------------------------------------------
// --version and help
// ---------------------------------------------------------------------------

#[test]
fn every_name_answers_version_with_itself_the_crate_and_the_version() {
    for name in ["fs.demo", "fsck.demo", "rust-fs-demo"] {
        for flag in ["--version", "-V"] {
            let r = respond(&[name, flag]);
            assert_eq!(
                r.stdout,
                format!("{name} (am-fs-demo) 1.2.3\n"),
                "{name} {flag}"
            );
            assert_eq!((r.stderr.as_str(), r.code), ("", 0), "{name} {flag}");
        }
    }
    // Reached through the entry point, the tool still answers as itself.
    let r = respond(&["rust-fs-demo", "fsck", "--version"]);
    assert_eq!(r.stdout, "fsck.demo (am-fs-demo) 1.2.3\n");
}

#[test]
fn help_is_on_stdout_with_status_0_and_carries_the_examples() {
    let r = respond(&["fs.demo", "--help"]);
    assert_eq!(r.code, 0);
    assert!(r.stdout.contains("Usage: fs.demo"), "{}", r.stdout);
    assert!(r.stdout.contains("Examples:"), "{}", r.stdout);
    assert!(r.stderr.is_empty());

    let r = respond(&["rust-fs-demo", "--help"]);
    assert_eq!(r.code, 0);
    for line in [
        "rust-fs-demo fs --help",
        "rust-fs-demo fsck --help",
        "rust-fs-demo doctor",
        "(the same program as `fs.demo`)",
    ] {
        assert!(r.stdout.contains(line), "no {line:?} in:\n{}", r.stdout);
    }
    // `generate` is for packaging, not for a person reading help.
    assert!(!r.stdout.contains("generate"), "{}", r.stdout);
}

#[test]
fn the_entry_points_examples_line_up_however_long_the_names_are() {
    // Every explanation starts in the same column: the fixed spacing one
    // copy of this plumbing used put `doctor`'s explanation somewhere else
    // as soon as a verb was longer than it expected.
    let help = cli::repo_command(&FAMILY).render_help().to_string();
    let rows: Vec<&str> = help
        .lines()
        .skip_while(|l| *l != "Examples:")
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .collect();
    assert_eq!(rows.len(), FAMILY.tools.len() + 1, "{help}");
    // Where each explanation starts: after the command, at least four
    // spaces, then the first character that is not a space.
    let columns: Vec<usize> = rows
        .iter()
        .map(|row| {
            let cmd_end = 2 + row[2..].find("    ").expect("two columns");
            let rest = &row[cmd_end..];
            cmd_end + rest.len() - rest.trim_start().len()
        })
        .collect();
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "explanations start in columns {columns:?}:\n{help}"
    );
    let longest = rows
        .iter()
        .map(|row| row[2..].find("    ").unwrap())
        .max()
        .unwrap();
    assert_eq!(columns[0], 2 + longest + 4, "{help}");
}

#[test]
fn a_bare_entry_point_prints_its_help_and_did_nothing() {
    let r = respond(&["rust-fs-demo"]);
    assert_eq!(r.code, 2);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.contains("Usage: rust-fs-demo"), "{}", r.stderr);
}

// ---------------------------------------------------------------------------
// Results and errors
// ---------------------------------------------------------------------------

#[test]
fn a_result_is_pretty_json_on_stdout_by_default() {
    let r = respond(&["fs.demo", "report"]);
    assert_eq!(
        r.stdout,
        "{\n  \"fs\": \"demo\",\n  \"label\": null,\n  \"demo\": {\n    \"blocks\": 8\n  }\n}\n"
    );
    assert_eq!((r.stderr.as_str(), r.code), ("", 0));
    // --json says so explicitly, and is the same.
    assert_eq!(respond(&["fs.demo", "report", "--json"]), r);
}

#[test]
fn text_is_for_a_person_and_the_last_format_flag_wins() {
    let r = respond(&["fs.demo", "--text", "report"]);
    assert_eq!(r.stdout, "fs: demo\nlabel: \ndemo.blocks: 8\n");
    // A subcommand's flag counts as much as the tool's, and the last wins.
    assert_eq!(respond(&["fs.demo", "report", "--text"]).stdout, r.stdout);
    assert_eq!(
        respond(&["fs.demo", "--text", "report", "--json"]).stdout,
        respond(&["fs.demo", "report"]).stdout
    );
    assert_eq!(
        respond(&["fs.demo", "--json", "report", "--text"]).stdout,
        r.stdout
    );
    assert_eq!(
        respond(&["fs.demo", "report", "--text", "--json"]).stdout,
        respond(&["fs.demo", "report"]).stdout
    );
    assert_eq!(
        respond(&["fs.demo", "custom-text", "--text"]).stdout,
        "a person's summary\n"
    );
}

#[test]
fn an_empty_text_form_prints_nothing_not_an_empty_line() {
    let r = respond(&["fs.demo", "silent", "--text"]);
    assert_eq!(r, cli::Response::default());
}

#[test]
fn raw_output_is_the_tools_own_and_the_plumbing_adds_nothing() {
    assert_eq!(respond(&["fs.demo", "raw"]), cli::Response::default());
}

#[test]
fn a_success_with_another_answer_keeps_its_status() {
    let r = respond(&["fs.demo", "corrected"]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.starts_with('{'), "{}", r.stdout);
}

#[test]
fn a_failure_is_a_structured_error_on_stderr_and_its_code_is_the_status() {
    let r = respond(&["fs.demo", "fail"]);
    assert_eq!(r.stdout, "");
    assert_eq!(
        r.stderr,
        "{\"error\": \"open disk.img: no such file\", \"code\": 1}\n"
    );
    assert_eq!(r.code, 1);

    let r = respond(&["fs.demo", "fail", "--text"]);
    assert_eq!(r.stderr, "fs.demo: open disk.img: no such file\n");
    assert_eq!(r.code, 1);

    // Reached through the entry point, the tool still names itself.
    let r = respond(&["rust-fs-demo", "fs", "fail", "--text"]);
    assert_eq!(r.stderr, "fs.demo: open disk.img: no such file\n");
}

#[test]
fn a_verb_this_crate_cannot_do_answers_3_and_says_so() {
    let r = respond(&["fs.demo", "unsupported"]);
    assert_eq!(
        r.stderr,
        "{\"error\": \"not implemented: resize\", \"code\": 3}\n"
    );
    assert_eq!(r.code, 3);
    let r = respond(&["fs.demo", "refuse"]);
    assert_eq!(
        r.stderr,
        "{\"error\": \"the format is read-only\", \"code\": 3}\n"
    );
    assert_eq!(r.code, 3);
}

#[test]
fn a_tool_specific_failure_status_is_kept() {
    let r = respond(&["fsck.demo", "x.img"]);
    assert_eq!(r.code, 4);
    assert!(r.stderr.contains("\"code\": 4}"), "{}", r.stderr);
}

#[test]
fn a_wrong_command_line_is_a_structured_error_with_status_2() {
    let r = respond(&["fs.demo", "--no-such-flag", "report"]);
    assert_eq!(r.code, 2);
    assert!(r.stdout.is_empty(), "{}", r.stdout);
    assert!(
        r.stderr.starts_with("{\"error\": \"") && r.stderr.ends_with("\"code\": 2}\n"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("--no-such-flag"), "{}", r.stderr);
    assert!(
        !r.stderr.contains("error: "),
        "the prefix is stripped: {}",
        r.stderr
    );

    // --text: clap's own message, for a person.
    let r = respond(&["fs.demo", "--text", "--no-such-flag", "report"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.starts_with("error: "), "{}", r.stderr);

    // The entry point too.
    let r = respond(&["rust-fs-demo", "no-such-verb"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("no-such-verb"), "{}", r.stderr);
}

#[test]
fn a_tool_with_its_own_usage_status_keeps_it() {
    let r = respond(&["fsck.demo"]);
    assert_eq!(r.code, 16, "{}", r.stderr);
    assert!(r.stderr.ends_with("\"code\": 16}\n"), "{}", r.stderr);
    let r = respond(&["fsck.demo", "--text"]);
    assert_eq!(r.code, 16);
}

// ---------------------------------------------------------------------------
// generate: names, man pages, completions
// ---------------------------------------------------------------------------

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("am-fs-core-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn generate_names_lists_every_dotted_name_to_link() {
    let r = respond(&["rust-fs-demo", "generate", "names"]);
    assert_eq!(r.stdout, "fs.demo\nfsck.demo\n");
    assert_eq!(r.code, 0);
}

#[test]
fn every_name_gets_a_man_page_in_its_section_and_each_subcommand_one_beside_it() {
    let share = scratch("man");
    let arg = share.to_string_lossy().into_owned();
    let r = respond(&["rust-fs-demo", "generate", "man", &arg]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    for page in [
        "man/man1/fs.demo.1",
        "man/man8/fsck.demo.8",
        "man/man1/rust-fs-demo.1",
        "man/man1/fs.demo-report.1",
        "man/man1/rust-fs-demo-doctor.1",
    ] {
        let text =
            std::fs::read_to_string(share.join(page)).unwrap_or_else(|e| panic!("{page}: {e}"));
        assert!(text.contains(".TH "), "{page}: not a man page");
        assert!(
            // Reported in the platform's own separators, which on Windows
            // are not the `/` the page list above is written in, and as a
            // JSON string, which doubles each of those backslashes.
            r.stdout.contains(
                &share
                    .join(page.split('/').collect::<PathBuf>())
                    .display()
                    .to_string()
                    .replace('\\', "\\\\")
            ),
            "{page} not reported:\n{}",
            r.stdout
        );
    }
    let page = std::fs::read_to_string(share.join("man/man1/fs.demo.1")).unwrap();
    assert!(page.contains("Examples:"), "{page}");
    assert!(
        page.contains("\"am-fs-demo 1.2.3\" rust-fs-demo"),
        "the title line: {page}"
    );
    // Not paged: a hidden subcommand, clap's `help`, the entry point's tool
    // verbs (documented once, under their dotted names) and `generate`.
    for absent in [
        "man/man1/fs.demo-hidden.1",
        "man/man1/fs.demo-help.1",
        "man/man1/rust-fs-demo-fs.1",
        "man/man1/rust-fs-demo-fsck.1",
        "man/man1/rust-fs-demo-generate.1",
    ] {
        assert!(!share.join(absent).exists(), "{absent} was written");
    }
    std::fs::remove_dir_all(&share).unwrap();
}

#[test]
fn every_name_gets_a_zsh_bash_and_fish_completion() {
    let share = scratch("completions");
    let arg = share.to_string_lossy().into_owned();
    let r = respond(&["rust-fs-demo", "generate", "completions", &arg]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    for name in ["fs.demo", "fsck.demo", "rust-fs-demo"] {
        let zsh = std::fs::read_to_string(share.join(format!("zsh/site-functions/_{name}")))
            .unwrap_or_else(|e| panic!("zsh {name}: {e}"));
        assert!(zsh.starts_with(&format!("#compdef {name}")), "zsh {name}");
        let bash =
            std::fs::read_to_string(share.join(format!("bash-completion/completions/{name}")))
                .unwrap_or_else(|e| panic!("bash {name}: {e}"));
        assert!(bash.contains("complete -F"), "bash {name}");
        let fish =
            std::fs::read_to_string(share.join(format!("fish/vendor_completions.d/{name}.fish")))
                .unwrap_or_else(|e| panic!("fish {name}: {e}"));
        assert!(fish.contains(&format!("complete -c {name}")), "fish {name}");
    }
    std::fs::remove_dir_all(&share).unwrap();
}

#[test]
fn completions_for_other_shells_go_under_a_directory_of_their_own() {
    use fs_core::cli::docs::completion_path;
    let share = Path::new("/s");
    assert_eq!(
        completion_path(share, docs::Shell::Elvish, "fs.demo"),
        Path::new("/s/elvish/fs.demo")
    );
    assert_eq!(docs::SHELLS.len(), 3);
}

#[test]
fn a_share_directory_that_cannot_be_written_is_a_structured_failure() {
    let dir = scratch("unwritable");
    let blocker = dir.join("a-file");
    std::fs::write(&blocker, b"").unwrap();
    let arg = blocker.to_string_lossy().into_owned();
    for what in ["man", "completions"] {
        let r = respond(&["rust-fs-demo", "generate", what, &arg]);
        assert_eq!(r.code, 1, "{what}");
        assert!(
            r.stderr
                .starts_with(&format!("{{\"error\": \"generate {what}: ")),
            "{}",
            r.stderr
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_through_the_entry_point_reports_and_exits_1_when_a_name_is_not_ours() {
    // The demo names are on nobody's PATH.
    let r = respond(&["rust-fs-demo", "doctor"]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert!(r.stdout.contains("\"ok\": false"), "{}", r.stdout);
    assert!(r.stdout.contains("\"status\": \"missing\""), "{}", r.stdout);
    let r = respond(&["rust-fs-demo", "doctor", "--text"]);
    assert!(
        r.stdout.contains("fs.demo: missing (not on PATH)"),
        "{}",
        r.stdout
    );
}

#[cfg(unix)]
mod on_path {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// An executable script at `path` with `body` after the shebang.
    fn script(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A program answering `--version` with `line`.
    fn answering(path: &Path, line: &str) {
        script(path, &format!("echo '{line}'"));
    }

    /// A directory where every demo name answers as ours.
    fn ours(tag: &str) -> PathBuf {
        let dir = scratch(tag);
        for tool in FAMILY.tools {
            answering(
                &dir.join(tool.name),
                &format!("{} (am-fs-demo) 1.2.3", tool.name),
            );
        }
        dir
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn finding(findings: &[doctor::Finding], name: &str) -> doctor::Finding {
        findings.iter().find(|f| f.name == name).unwrap().clone()
    }

    fn json(outcome: &Outcome) -> String {
        outcome.report.as_ref().unwrap().to_pretty()
    }

    #[test]
    fn every_name_ours_is_ok_with_status_0() {
        let dir = ours("all-ours");
        let outcome = doctor::run_path(&FAMILY, &path_of(&[&dir]));
        assert_eq!(outcome.code, 0, "{}", json(&outcome));
        let report = json(&outcome);
        assert!(report.contains("\"ok\": true"), "{report}");
        assert!(report.contains("\"crate\": \"am-fs-demo\""), "{report}");
        assert!(!report.contains("\"fix\": \""), "{report}");
        assert!(
            outcome
                .text
                .as_deref()
                .unwrap()
                .ends_with("every tool is am-fs-demo 1.2.3"),
            "{:?}",
            outcome.text
        );
    }

    #[test]
    fn a_missing_name_says_how_to_install_it() {
        let empty = scratch("empty");
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&empty])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Missing);
        let fix = f.fix.unwrap();
        assert!(
            fix.starts_with("fs.demo is not on PATH. Install it: "),
            "{fix}"
        );
        assert!(
            fix.contains(
                "`chore cli:install` from a checkout, or `brew install example/tap/rust-fs-demo`"
            ),
            "{fix}"
        );
    }

    #[test]
    fn a_foreign_program_ahead_of_ours_says_which_path_entry_to_move() {
        let theirs = scratch("foreign");
        answering(&theirs.join("fs.demo"), "fs.demo 1.0 (another package)");
        let mine = ours("foreign-ours");
        let findings = doctor::diagnose_path(&FAMILY, &path_of(&[&theirs, &mine]));
        let f = finding(&findings, "fs.demo");
        assert_eq!(f.status, doctor::Status::Foreign);
        assert_eq!(f.path.as_deref(), Some(theirs.join("fs.demo").as_path()));
        assert_eq!(f.version.as_deref(), Some("fs.demo 1.0 (another package)"));
        assert_eq!(f.shadowed, vec![mine.join("fs.demo")]);
        let fix = f.fix.unwrap();
        assert!(
            fix.contains(&format!(
                "put {} before {} on PATH",
                mine.display(),
                theirs.display()
            )),
            "{fix}"
        );
        assert!(fix.contains("is not am-fs-demo's fs.demo"), "{fix}");
        // The other name is ours and says nothing.
        assert_eq!(finding(&findings, "fsck.demo").status, doctor::Status::Ours);
    }

    #[test]
    fn a_foreign_program_with_nothing_of_ours_behind_it_says_to_install_ours() {
        let theirs = scratch("foreign-alone");
        answering(&theirs.join("fs.demo"), "fs.demo (another-crate) 1.2.3");
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&theirs])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Foreign);
        let fix = f.fix.unwrap();
        assert!(
            fix.contains(&format!("install ours (`chore cli:install` from a checkout, or `brew install example/tap/rust-fs-demo`) in a directory before {} on PATH", theirs.display())),
            "{fix}"
        );
    }

    #[test]
    fn our_name_under_another_tools_answer_is_foreign() {
        // Our crate and version, but it says it is a different tool.
        let dir = scratch("wrong-tool");
        answering(&dir.join("fs.demo"), "fsck.demo (am-fs-demo) 1.2.3");
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&dir])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Foreign);
    }

    #[test]
    fn our_program_at_another_version_is_stale() {
        let old = scratch("stale");
        answering(&old.join("fs.demo"), "fs.demo (am-fs-demo) 0.0.1");
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&old])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Stale);
        assert!(f.fix.unwrap().contains("is am-fs-demo 0.0.1, not 1.2.3"));
    }

    #[test]
    fn a_program_in_a_homebrew_cellar_names_the_formula_to_unlink() {
        let prefix = scratch("brew");
        let real = prefix.join("Cellar/other-fs-tools/1.0/bin/fs.demo");
        answering(&real, "fs.demo (other-fs-tools) 1.0.0");
        let bin = prefix.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&real, bin.join("fs.demo")).unwrap();
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&bin])),
            "fs.demo",
        );
        assert_eq!(f.formula.as_deref(), Some("other-fs-tools"));
        assert!(f.fix.unwrap().starts_with(&format!(
            "{} is not am-fs-demo's fs.demo: `brew unlink other-fs-tools`, or ",
            bin.join("fs.demo").display()
        )));
        assert_eq!(
            doctor::cellar_formula(&bin.join("fs.demo")),
            Some("other-fs-tools".into())
        );
        assert_eq!(doctor::cellar_formula(&prefix.join("nothing-here")), None);
    }

    #[test]
    fn a_file_that_is_not_executable_is_not_on_path() {
        let dir = scratch("not-executable");
        std::fs::write(
            dir.join("fs.demo"),
            "#!/bin/sh\necho 'fs.demo (am-fs-demo) 1.2.3'\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("fsck.demo")).unwrap();
        let findings = doctor::diagnose_path(&FAMILY, &path_of(&[&dir]));
        assert_eq!(
            finding(&findings, "fs.demo").status,
            doctor::Status::Missing
        );
        assert_eq!(
            finding(&findings, "fsck.demo").status,
            doctor::Status::Missing
        );
    }

    #[test]
    fn later_programs_are_not_run_when_the_winner_is_ours() {
        // A CI runner may stub the reference tools to fail loudly; asking
        // them anything when ours already wins is a side effect for nothing.
        let mine = ours("ours-first");
        let stub = scratch("stub");
        let tripwire = stub.join("ran");
        script(
            &stub.join("fs.demo"),
            &format!("touch '{}'\nexit 1", tripwire.display()),
        );
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&mine, &stub])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Ours);
        assert_eq!(f.shadowed, vec![stub.join("fs.demo")]);
        assert!(!tripwire.exists(), "the shadowed program was run");
    }

    #[test]
    fn a_long_answer_is_read_while_the_program_runs() {
        // A program that answers `--version` and then keeps writing, more
        // than a pipe holds. Read only after it exits, it blocks on the full
        // pipe, runs out the probe's time and is taken for someone else's.
        let dir = scratch("chatty");
        script(
            &dir.join("fs.demo"),
            "echo 'fs.demo (am-fs-demo) 1.2.3'\nhead -c 1048576 /dev/zero",
        );
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&dir])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Ours, "{f:?}");
        assert_eq!(f.version.as_deref(), Some("fs.demo (am-fs-demo) 1.2.3"));
    }

    #[test]
    fn a_program_that_never_answers_is_given_up_on_and_is_foreign() {
        let dir = scratch("silent");
        script(&dir.join("fs.demo"), "exec sleep 30");
        let f = finding(
            &doctor::diagnose_path(&FAMILY, &path_of(&[&dir])),
            "fs.demo",
        );
        assert_eq!(f.status, doctor::Status::Foreign);
        assert_eq!(f.version, None);
    }

    #[test]
    fn the_text_report_is_for_a_person_and_keeps_the_fix() {
        let theirs = scratch("text");
        answering(&theirs.join("fs.demo"), "something else entirely");
        let mine = ours("text-ours");
        let outcome = doctor::run_path(&FAMILY, &path_of(&[&theirs, &mine]));
        assert_eq!(outcome.code, 1);
        let text = outcome.text.unwrap();
        assert!(text.contains("fs.demo: foreign ("), "{text}");
        assert!(text.contains("  also on PATH, not run: "), "{text}");
        assert!(text.contains("  fix: "), "{text}");
        assert!(text.contains("fsck.demo: ours ("), "{text}");
        assert!(
            text.ends_with("some tools on PATH are not this program; see the fixes above"),
            "{text}"
        );
        assert!(!text.contains('{'), "{text}");
    }
}

// ---------------------------------------------------------------------------
// Json, Outcome, CliError
// ---------------------------------------------------------------------------

#[test]
fn every_scalar_converts_and_renders() {
    let v = Json::object([
        ("t", Json::from(true)),
        ("f", Json::from(false)),
        ("u64", Json::from(u64::MAX)),
        ("u32", Json::from(7u32)),
        ("u16", Json::from(9u16)),
        ("i64", Json::from(-5i64)),
        ("some", Json::from(Some("x"))),
        ("none", Json::from(None::<String>)),
        ("owned", Json::from(String::from("y"))),
        ("list", Json::from(vec![1u64, 2])),
        ("empty_list", Json::Arr(vec![])),
        ("empty_obj", Json::Obj(vec![])),
    ]);
    assert_eq!(
        v.to_compact(),
        "{\"t\": true, \"f\": false, \"u64\": 18446744073709551615, \"u32\": 7, \"u16\": 9, \
         \"i64\": -5, \"some\": \"x\", \"none\": null, \"owned\": \"y\", \"list\": [1, 2], \
         \"empty_list\": [], \"empty_obj\": {}}"
    );
    assert!(
        v.to_pretty().contains("\"list\": [\n    1,\n    2\n  ],"),
        "{}",
        v.to_pretty()
    );
    assert!(
        v.to_pretty().contains("\"empty_list\": [],"),
        "{}",
        v.to_pretty()
    );
    assert_eq!(v.get("u32"), Some(&Json::UInt(7)));
    assert_eq!(v.get("nope"), None);
    assert_eq!(Json::from(1u64).get("x"), None);
}

#[test]
fn text_renders_arrays_one_per_line_and_nested_values_inline() {
    assert_eq!(Json::from(vec!["a", "b"]).to_text(), "a\nb");
    assert_eq!(Json::from("bare").to_text(), "bare");
    assert_eq!(Json::Null.to_text(), "");
    let v = Json::object([
        ("list", Json::from(vec![1u64, 2])),
        ("i", Json::from(-1i64)),
        ("b", Json::from(true)),
    ]);
    assert_eq!(v.to_text(), "list: 1,2\ni: -1\nb: true");
    let rows = Json::Arr(vec![Json::object([
        ("name", Json::from("a")),
        ("size", Json::from(3u64)),
    ])]);
    assert_eq!(rows.to_text(), "name=a size=3");
}

#[test]
fn outcomes_and_errors_carry_what_they_were_given() {
    assert_eq!(Outcome::done().report, None);
    assert_eq!(Outcome::done().code, 0);
    let e = CliError::usage("bad").with_code(16);
    assert_eq!((e.message.as_str(), e.code), ("bad", 16));
    assert_eq!(CliError::usage("bad").code, output::EXIT_USAGE);
    assert_eq!(CliError::failed("x").code, output::EXIT_FAILED);
    assert_eq!(
        CliError::not_implemented("x").code,
        output::EXIT_UNSUPPORTED
    );
    assert_eq!(
        CliError::failed("x").to_json().to_compact(),
        "{\"error\": \"x\", \"code\": 1}"
    );
}

#[test]
fn render_is_what_finish_would_print() {
    let r = output::render("t", cli::Format::Text, Err(CliError::failed("boom")));
    assert_eq!(r.stderr, "t: boom\n");
    let r = output::render("t", cli::Format::Json, Ok(Outcome::report(Json::from("v"))));
    assert_eq!(r.stdout, "\"v\"\n");
}
