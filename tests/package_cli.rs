//! `scripts/package-cli.sh`, the family's release tarball, run the way every
//! repository runs it: `scripts/core.sh package-cli VERSION LABEL [DIR]` from
//! a CALLER tree that is not this one.
//!
//! Ten repositories once carried ten different copies of the script and ten
//! copies of its test, and the copies held three policies for which names go
//! in the tarball (one shipped a single tool for a whole release). This is
//! the one test of the one script. Each caller tree is built under this
//! repository's `tmp/`, never the OS temporary directory, and removed
//! afterwards. Its "built binary" is a stand-in that behaves as a binary
//! built on this crate's `cli` module does -- `generate names|man|
//! completions`, and `--version` answered as the name it was started under
//! -- or misbehaves in one way, so each refusal is proved on its own. The
//! callers' own CI jobs run the same script against their real binaries.
//!
//! UNIX ONLY, AND NOT A SKIP: a release tarball is an install prefix of
//! relative symlinks, packaged on the darwin and linux release legs. The
//! Windows mode (a `windows-` label: a `.zip` whose dotted names are copies
//! of `bin/<repo>.exe`, #232) is tested here too, from a stand-in named
//! `.exe`, because the script itself is bash on every platform; the run on
//! a real Windows runner, against a real binary, is a caller's CI job.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn printed(output: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const CRATE: &str = "am-fs-example";
const REPO: &str = "rust-fs-example";

/// The manifest an erofs-shaped repository writes: two dotted names, one in
/// section 8, and one licence file.
const TABLE: &str = "[package.metadata.package-cli]\n\
                     names = { \"mkfs.example\" = 8, \"fs.example\" = 1 }\n\
                     licenses = [\"LICENSE\"]\n";

/// How the stand-in binary behaves. The default is a good build.
#[derive(Clone)]
struct Stub {
    /// What `generate names` prints, one per line.
    names: &'static str,
    /// `name:section` for every page `generate man` writes.
    pages: &'static str,
    /// Every name `generate completions` writes the three shells' files for.
    completions: &'static str,
    /// A path under SHARE that `generate man` also writes, or "".
    stray: &'static str,
    /// What `--version` reports after `<name> (<crate>)`.
    version: &'static str,
    /// `--help`'s exit status.
    help: u8,
}

impl Default for Stub {
    fn default() -> Self {
        Stub {
            names: "mkfs.example fs.example",
            // A page per name in its section, and one per subcommand beside
            // it, as am-fs-core's `cli` module writes them.
            pages: "mkfs.example:8 fs.example:1 fs.example-ls:1 rust-fs-example:1 rust-fs-example-doctor:1",
            completions: "mkfs.example fs.example rust-fs-example",
            stray: "",
            version: "9.9.9",
            help: 0,
        }
    }
}

/// A throwaway repository under this one's `tmp/`, removed on drop.
struct Caller {
    root: PathBuf,
}

impl Caller {
    fn new(tag: &str) -> Self {
        Caller::with(tag, REPO, TABLE)
    }

    /// A caller whose repository (and so binary) is `repo`, with `table` as
    /// its `[package.metadata.package-cli]`.
    fn with(tag: &str, repo_name: &str, table: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = repo().join("tmp").join(format!(
            "package-cli-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        // The guard exists before anything that can panic, so a failed setup
        // still removes the tree.
        let caller = Caller { root };
        fs::create_dir_all(caller.root.join("scripts")).unwrap();
        fs::create_dir_all(caller.root.join("packaging")).unwrap();
        fs::copy(
            repo().join("scripts").join("core.sh"),
            caller.root.join("scripts").join("core.sh"),
        )
        .expect("rust-fs-core ships scripts/core.sh for its consumers");
        fs::write(
            caller.root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{CRATE}\"\nversion = \"9.9.9\"\nedition = \"2021\"\n\
                 repository = \"https://github.com/example-org/{repo_name}\"\n\n\
                 [[bin]]\nname = \"{repo_name}\"\npath = \"src/main.rs\"\n\n{table}"
            ),
        )
        .unwrap();
        for licence in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"] {
            fs::write(caller.root.join(licence), format!("{licence} text\n")).unwrap();
        }
        fs::write(
            caller.root.join("packaging/CAVEATS"),
            "mkfs.example shadows another package's; `brew unlink` it first.\n",
        )
        .unwrap();
        caller
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Writes the stand-in binary as `<dir>/<binary>` and returns `<dir>`.
    fn build(&self, dir: &str, binary: &str, stub: &Stub) -> PathBuf {
        let dir = self.root.join(dir);
        fs::create_dir_all(&dir).unwrap();
        let Stub {
            names,
            pages,
            completions,
            stray,
            version,
            help,
        } = stub;
        let script = format!(
            r##"#!/usr/bin/env bash
me="$(basename "$0")"
me="${{me%.exe}}"
case "$1" in
    --help) echo "Usage: $me"; exit {help} ;;
    --version) echo "$me ({CRATE}) {version}" ;;
    generate)
        case "$2" in
            names) printf '%s\n' {names} ;;
            man)
                echo "["
                for p in {pages}; do
                    n="${{p%%:*}}"; s="${{p##*:}}"
                    mkdir -p "$3/man/man$s"
                    echo ".TH $n $s" >"$3/man/man$s/$n.$s"
                    echo "  \"$3/man/man$s/$n.$s\","
                done
                if [ -n "{stray}" ]; then mkdir -p "$(dirname "$3/{stray}")"; echo x >"$3/{stray}"; fi
                echo "]" ;;
            completions)
                mkdir -p "$3/zsh/site-functions" "$3/bash-completion/completions" "$3/fish/vendor_completions.d"
                for n in {completions}; do
                    echo "#compdef $n" >"$3/zsh/site-functions/_$n"
                    echo "complete -F _$n $n" >"$3/bash-completion/completions/$n"
                    echo "complete -c $n" >"$3/fish/vendor_completions.d/$n.fish"
                done ;;
        esac ;;
    *) exit 2 ;;
esac
"##
        );
        let path = dir.join(binary);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    /// `bash <caller>/scripts/core.sh package-cli ARGS...` from a fresh
    /// output directory, which first holds `stale` (a previous run's
    /// tarball) when it is not empty.
    fn package(&self, args: &[&str], stale: &str, env: &[(&str, String)]) -> Packaged {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let out = self
            .root
            .join(format!("out-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&out).unwrap();
        if !stale.is_empty() {
            fs::write(out.join(stale), "a previous run's tarball\n").unwrap();
        }
        let mut command = Command::new("bash");
        command
            .current_dir(&out)
            .arg(self.root.join("scripts/core.sh"))
            .arg("package-cli")
            .args(args)
            .env("FS_CORE_ROOT", repo());
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        Packaged { out, output }
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Packaged {
    out: PathBuf,
    output: Output,
}

impl Packaged {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).to_string()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).to_string()
    }

    fn tarballs(&self) -> Vec<String> {
        fs::read_dir(&self.out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tar.gz") || n.ends_with(".zip"))
            .collect()
    }

    /// The packaged tarball, which must be the one file named on stdout.
    fn tarball(&self) -> PathBuf {
        assert!(self.output.status.success(), "{}", printed(&self.output));
        let name = self.stdout();
        let name = name.trim_end_matches('\n');
        assert!(
            !name.is_empty() && !name.contains('\n'),
            "stdout is the tarball's name and nothing else: {}",
            printed(&self.output)
        );
        let path = self.out.join(name);
        assert!(path.is_file(), "{} names no file", name);
        path
    }

    /// A refusal: a failure status, nothing on stdout, no tarball left --
    /// not even the stale one -- and `why` in what it said.
    fn refused(&self, case: &str, why: &str) -> Result<(), String> {
        let mut wrong = Vec::new();
        if self.output.status.success() {
            wrong.push("it succeeded");
        }
        if !self.stdout().is_empty() {
            wrong.push("it printed on stdout");
        }
        if !self.tarballs().is_empty() {
            wrong.push("a tarball was left behind");
        }
        if !self.stderr().contains(why) {
            wrong.push("it did not say why");
        }
        if wrong.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{case}: {} (expected `{why}`)\n{}",
                wrong.join(", "),
                printed(&self.output)
            ))
        }
    }
}

/// Files and symlinks in the tarball, directories dropped, in byte order --
/// Rust's `sort` is the C locale's, whatever the environment says.
fn members(tarball: &Path) -> Vec<String> {
    let out = Command::new("tar")
        .arg("-tzf")
        .arg(tarball)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    let mut members: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_start_matches("./").to_string())
        .filter(|l| !l.is_empty() && !l.ends_with('/'))
        .collect();
    members.sort();
    members
}

fn unpack(caller: &Caller, tarball: &Path) -> PathBuf {
    let into = caller.root.join("unpacked");
    let _ = fs::remove_dir_all(&into);
    fs::create_dir_all(&into).unwrap();
    let out = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&into)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    into
}

/// The files in a `.zip`, directories dropped, in byte order. Read by
/// Python's zipfile, the reader package-cli.sh writes them with on every
/// platform; `unzip` is not on a Windows runner.
fn zip_members(zip: &Path) -> Vec<String> {
    let out = Command::new("python3")
        .args([
            "-c",
            "import sys, zipfile\nfor n in zipfile.ZipFile(sys.argv[1]).namelist(): print(n)",
        ])
        .arg(zip)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    let mut members: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty() && !l.ends_with('/'))
        .map(str::to_owned)
        .collect();
    members.sort();
    members
}

/// Unpacks a `.zip` as a Windows user's tool would: regular files only, no
/// mode bits. The executable bit is set afterwards so this host can run the
/// stand-in; Windows runs a `.exe` by its name.
fn unzip(caller: &Caller, zip: &Path) -> PathBuf {
    let into = caller.root.join("unzipped");
    let _ = fs::remove_dir_all(&into);
    fs::create_dir_all(&into).unwrap();
    let out = Command::new("python3")
        .args([
            "-c",
            "import sys, zipfile\nzipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])",
        ])
        .arg(zip)
        .arg(&into)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    for entry in fs::read_dir(into.join("bin")).unwrap() {
        let path = entry.unwrap().path();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    into
}

fn sorted(list: &[&str]) -> Vec<String> {
    let mut list: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    list.sort();
    list
}

fn all_ok(results: Vec<Result<(), String>>) {
    let failures: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn a_good_build_is_exactly_the_install_prefix() {
    let caller = Caller::new("good");
    let built = caller.build("target/release", REPO, &Stub::default());
    let run = caller.package(&["9.9.9", "darwin-arm64"], "", &[]);
    let tarball = run.tarball();
    assert_eq!(
        tarball.file_name().unwrap(),
        "am-fs-example-9.9.9-darwin-arm64.tar.gz",
        "the tarball is <crate>-<version>-<label>.tar.gz"
    );
    assert_eq!(
        members(&tarball),
        sorted(&[
            "LICENSE",
            "bin/fs.example",
            "bin/mkfs.example",
            "bin/rust-fs-example",
            "share/bash-completion/completions/fs.example",
            "share/bash-completion/completions/mkfs.example",
            "share/bash-completion/completions/rust-fs-example",
            "share/fish/vendor_completions.d/fs.example.fish",
            "share/fish/vendor_completions.d/mkfs.example.fish",
            "share/fish/vendor_completions.d/rust-fs-example.fish",
            "share/man/man1/fs.example-ls.1",
            "share/man/man1/fs.example.1",
            "share/man/man1/rust-fs-example-doctor.1",
            "share/man/man1/rust-fs-example.1",
            "share/man/man8/mkfs.example.8",
            "share/rust-fs-example/CAVEATS",
            "share/zsh/site-functions/_fs.example",
            "share/zsh/site-functions/_mkfs.example",
            "share/zsh/site-functions/_rust-fs-example",
        ]),
    );

    let u = unpack(&caller, &tarball);
    let binary = u.join("bin").join(REPO);
    assert!(
        !fs::symlink_metadata(&binary)
            .unwrap()
            .file_type()
            .is_symlink(),
        "bin/{REPO} is the real file"
    );
    assert_eq!(
        fs::read(&binary).unwrap(),
        fs::read(built.join(REPO)).unwrap(),
        "bin/{REPO} is the built binary"
    );
    for name in ["mkfs.example", "fs.example"] {
        assert_eq!(
            fs::read_link(u.join("bin").join(name)).unwrap(),
            Path::new(REPO),
            "bin/{name} is a relative symlink to {REPO}"
        );
        let answer = Command::new(u.join("bin").join(name))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&answer.stdout).trim(),
            format!("{name} ({CRATE}) 9.9.9"),
            "bin/{name} answers as itself through its link"
        );
    }
    assert_eq!(
        fs::read(u.join("share").join(REPO).join("CAVEATS")).unwrap(),
        fs::read(caller.root.join("packaging/CAVEATS")).unwrap()
    );
    assert_eq!(
        fs::read(u.join("LICENSE")).unwrap(),
        fs::read(caller.root.join("LICENSE")).unwrap()
    );
}

/// A `windows-` label packages for Windows: a `.zip`, not a tarball, whose
/// binary is `bin/<repo>.exe` and whose dotted names are byte-identical
/// COPIES of it, `bin/<name>.exe`. A symlink needs Developer Mode or an
/// administrator on Windows, and a zip cannot hold one anyway; the binary
/// answers as the name it was started under, `.exe` dropped. The pages,
/// completions, CAVEATS and licences are the same as every other leg's.
#[test]
fn a_windows_build_is_a_zip_of_copies_not_symlinks() {
    let caller = Caller::new("windows");
    let exe = format!("{REPO}.exe");
    let built = caller.build("target/release", &exe, &Stub::default());
    let mut results = Vec::new();
    for label in ["windows-x86_64", "windows-arm64"] {
        let run = caller.package(&["9.9.9", label], "", &[]);
        if !run.output.status.success() {
            results.push(Err(format!("{label}: {}", printed(&run.output))));
            continue;
        }
        let zip = run.tarball();
        assert_eq!(
            zip.file_name().unwrap().to_string_lossy(),
            format!("am-fs-example-9.9.9-{label}.zip"),
            "the asset is <crate>-<version>-<label>.zip"
        );
        assert_eq!(run.tarballs().len(), 1, "a zip and no tarball beside it");
        assert_eq!(
            zip_members(&zip),
            sorted(&[
                "LICENSE",
                "bin/fs.example.exe",
                "bin/mkfs.example.exe",
                "bin/rust-fs-example.exe",
                "share/bash-completion/completions/fs.example",
                "share/bash-completion/completions/mkfs.example",
                "share/bash-completion/completions/rust-fs-example",
                "share/fish/vendor_completions.d/fs.example.fish",
                "share/fish/vendor_completions.d/mkfs.example.fish",
                "share/fish/vendor_completions.d/rust-fs-example.fish",
                "share/man/man1/fs.example-ls.1",
                "share/man/man1/fs.example.1",
                "share/man/man1/rust-fs-example-doctor.1",
                "share/man/man1/rust-fs-example.1",
                "share/man/man8/mkfs.example.8",
                "share/rust-fs-example/CAVEATS",
                "share/zsh/site-functions/_fs.example",
                "share/zsh/site-functions/_mkfs.example",
                "share/zsh/site-functions/_rust-fs-example",
            ]),
            "{label}"
        );
        let u = unzip(&caller, &zip);
        let binary = fs::read(built.join(&exe)).unwrap();
        for name in [REPO, "mkfs.example", "fs.example"] {
            let path = u.join("bin").join(format!("{name}.exe"));
            assert_eq!(
                fs::read(&path).unwrap(),
                binary,
                "{label}: bin/{name}.exe is a copy of the built {exe}"
            );
            let answer = Command::new(&path).arg("--version").output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&answer.stdout).trim(),
                format!("{name} ({CRATE}) 9.9.9"),
                "{label}: bin/{name}.exe answers as {name}"
            );
        }
        results.push(Ok(()));
    }
    all_ok(results);
}

/// The Windows mode refuses what every leg refuses, and leaves no zip --
/// not even a previous run's -- and a Windows build is looked for as
/// `<repo>.exe`, which is what cargo writes there.
#[test]
fn a_wrong_windows_build_is_refused_and_leaves_no_zip() {
    let caller = Caller::new("windows-refused");
    let stale = "am-fs-example-9.9.9-windows-x86_64.zip";
    let exe = format!("{REPO}.exe");
    let mut results = Vec::new();
    let cases: Vec<(&str, Stub, &str)> = vec![
        (
            "a binary that forgets a name",
            Stub { names: "mkfs.example", ..Stub::default() },
            "generate names lists [mkfs.example]",
        ),
        (
            "a binary reporting a version other than the tag's",
            Stub { version: "1.0.0", ..Stub::default() },
            "--version says 'rust-fs-example (am-fs-example) 1.0.0', expected 'rust-fs-example (am-fs-example) 9.9.9'",
        ),
    ];
    for (i, (case, stub, why)) in cases.into_iter().enumerate() {
        let dir = format!("build-{i}");
        caller.build(&dir, &exe, &stub);
        let target = caller.root.join(&dir);
        let run = caller.package(
            &["9.9.9", "windows-x86_64", target.to_str().unwrap()],
            stale,
            &[],
        );
        results.push(run.refused(case, why));
    }
    // Built without the `.exe`, as a unix leg's binary is named.
    caller.build("unix-named", REPO, &Stub::default());
    let target = caller.root.join("unix-named");
    let run = caller.package(
        &["9.9.9", "windows-x86_64", target.to_str().unwrap()],
        stale,
        &[],
    );
    results.push(run.refused("a binary without .exe", "no built rust-fs-example.exe at"));
    all_ok(results);
}

/// The names, sections, licences and CAVEATS path are each repository's own,
/// read from its Cargo.toml -- here the shape of an image crate's (one name,
/// section 1, no section 8 at all) with an NTFS-style pair of licences and
/// the CAVEATS somewhere else -- and nothing about them is written in the
/// script.
#[test]
fn what_ships_is_read_from_the_callers_manifest() {
    let caller = Caller::with(
        "manifest",
        "rust-img-other",
        "[package.metadata.package-cli]\n\
         names = { \"img.other\" = 1 }\n\
         licenses = [\"LICENSE-MIT\", \"LICENSE-APACHE\"]\n\
         caveats = \"dist/CAVEATS\"\n",
    );
    caller.write("dist/CAVEATS", "one line\n");
    caller.build(
        "build/out",
        "rust-img-other",
        &Stub {
            names: "img.other",
            pages: "img.other:1 img.other-info:1 rust-img-other:1",
            completions: "img.other rust-img-other",
            version: "1.2.3-rc.1",
            ..Stub::default()
        },
    );
    // A relative TARGET-DIR is taken from the current directory, as the
    // callers' CI jobs pass it (`../../target/release` from tmp/<dir>).
    let run = caller.package(&["1.2.3-rc.1", "linux-x86_64", "../build/out"], "", &[]);
    let tarball = run.tarball();
    assert_eq!(
        tarball.file_name().unwrap(),
        "am-fs-example-1.2.3-rc.1-linux-x86_64.tar.gz"
    );
    assert_eq!(
        members(&tarball),
        sorted(&[
            "LICENSE-APACHE",
            "LICENSE-MIT",
            "bin/img.other",
            "bin/rust-img-other",
            "share/bash-completion/completions/img.other",
            "share/bash-completion/completions/rust-img-other",
            "share/fish/vendor_completions.d/img.other.fish",
            "share/fish/vendor_completions.d/rust-img-other.fish",
            "share/man/man1/img.other-info.1",
            "share/man/man1/img.other.1",
            "share/man/man1/rust-img-other.1",
            "share/rust-img-other/CAVEATS",
            "share/zsh/site-functions/_img.other",
            "share/zsh/site-functions/_rust-img-other",
        ]),
    );
    let u = unpack(&caller, &tarball);
    assert_eq!(
        fs::read(u.join("share/rust-img-other/CAVEATS")).unwrap(),
        b"one line\n"
    );
}

/// Two copies compared the binary's list with the written one in the order
/// each happened to be in, and refused a correct build that listed them the
/// other way round; and a bare `sort` collates by locale. The names are a
/// set, and every comparison is made in the C locale whatever the caller's
/// environment says.
#[test]
fn the_order_and_locale_the_names_arrive_in_do_not_matter() {
    let caller = Caller::new("order");
    caller.build(
        "target/release",
        REPO,
        &Stub {
            names: "fs.example mkfs.example",
            ..Stub::default()
        },
    );
    let mut results = Vec::new();
    for locale in ["C", "C.UTF-8", "en_US.UTF-8", "en_GB.UTF-8"] {
        let run = caller.package(
            &["9.9.9", "darwin-arm64"],
            "",
            &[("LC_ALL", locale.to_string()), ("LANG", locale.to_string())],
        );
        results.push(if run.output.status.success() {
            Ok(())
        } else {
            Err(format!("LC_ALL={locale}: {}", printed(&run.output)))
        });
    }
    all_ok(results);
}

/// Each way a build can be wrong is refused: nothing on stdout, and no
/// tarball left behind -- not even the one a previous run left under the
/// same name, which a caller could otherwise take for this run's.
#[test]
fn each_way_a_build_can_be_wrong_is_refused_and_leaves_no_tarball() {
    let caller = Caller::new("refused");
    let stale = "am-fs-example-9.9.9-darwin-arm64.tar.gz";
    let good = Stub::default();
    let cases: Vec<(&str, Stub, &str)> = vec![
        (
            "a binary that forgets a name",
            Stub { names: "mkfs.example", ..good.clone() },
            "generate names lists [mkfs.example], but Cargo.toml's [package.metadata.package-cli] names [fs.example mkfs.example]",
        ),
        (
            "a binary that lists a name nobody declared",
            Stub { names: "mkfs.example fs.example fsck.example", ..good.clone() },
            "generate names lists [fs.example fsck.example mkfs.example]",
        ),
        (
            "a binary that lists no names",
            Stub { names: "", ..good.clone() },
            "generate names listed no tool names",
        ),
        (
            "a binary listing a name that is a path",
            Stub { names: "../mkfs.example fs.example", ..good.clone() },
            "listed '../mkfs.example', which is not a tool name",
        ),
        (
            "a binary listing its own name",
            Stub { names: "rust-fs-example fs.example", ..good.clone() },
            "listed 'rust-fs-example', which is not a tool name",
        ),
        (
            "a binary whose --help fails",
            Stub { help: 1, ..good.clone() },
            "--help failed",
        ),
        (
            "a binary reporting a version other than the tag's",
            Stub { version: "1.0.0", ..good.clone() },
            "--version says 'rust-fs-example (am-fs-example) 1.0.0', expected 'rust-fs-example (am-fs-example) 9.9.9'",
        ),
        (
            "a binary that writes no page for one name",
            Stub { pages: "mkfs.example:8 rust-fs-example:1", ..good.clone() },
            "no share/man/man1/fs.example.1 for fs.example",
        ),
        (
            "a binary that puts a name's page in the wrong section",
            Stub { pages: "mkfs.example:1 fs.example:1 rust-fs-example:1", ..good.clone() },
            "share/man/man1/mkfs.example.1",
        ),
        (
            "a binary that writes a page in a section no name has",
            Stub { pages: "mkfs.example:8 fs.example:1 fs.example-ls:8 rust-fs-example:1", ..good.clone() },
            "outside the install layout: share/man/man8/fs.example-ls.8",
        ),
        (
            "a binary that writes no completions for one name",
            Stub { completions: "mkfs.example rust-fs-example", ..good.clone() },
            "for fs.example",
        ),
        (
            "a binary that writes completions for a name not shipped",
            Stub { completions: "mkfs.example fs.example fsck.example rust-fs-example", ..good.clone() },
            "share/bash-completion/completions/fsck.example",
        ),
        (
            "a binary that writes a file outside the layout",
            Stub { stray: "elvish/lib/fs.example.elv", ..good.clone() },
            "outside the install layout: share/elvish/lib/fs.example.elv",
        ),
    ];
    let mut results = Vec::new();
    for (i, (case, stub, why)) in cases.into_iter().enumerate() {
        let dir = format!("build-{i}");
        caller.build(&dir, REPO, &stub);
        let target = caller.root.join(&dir);
        let run = caller.package(
            &["9.9.9", "darwin-arm64", target.to_str().unwrap()],
            stale,
            &[],
        );
        results.push(run.refused(case, why));
    }
    let run = caller.package(
        &[
            "9.9.9",
            "darwin-arm64",
            caller.root.join("nowhere").to_str().unwrap(),
        ],
        stale,
        &[],
    );
    results.push(run.refused("a missing binary", "no built rust-fs-example at"));
    all_ok(results);
}

/// Spoils one of a caller's files.
type Spoil = dyn Fn(&Caller);

#[test]
fn the_callers_caveats_and_licences_are_checked() {
    let mut results = Vec::new();
    let stale = "am-fs-example-9.9.9-darwin-arm64.tar.gz";
    let cases: [(&str, &Spoil, &str); 4] = [
        (
            "missing CAVEATS",
            &|c| fs::remove_file(c.root.join("packaging/CAVEATS")).unwrap(),
            "packaging/CAVEATS is missing or empty",
        ),
        (
            "empty CAVEATS",
            &|c| c.write("packaging/CAVEATS", ""),
            "packaging/CAVEATS is missing or empty",
        ),
        (
            "five lines of CAVEATS",
            &|c| c.write("packaging/CAVEATS", "1\n2\n3\n4\n5\n"),
            "packaging/CAVEATS is 5 lines",
        ),
        (
            "a missing licence",
            &|c| fs::remove_file(c.root.join("LICENSE")).unwrap(),
            "the licence LICENSE is missing or empty",
        ),
    ];
    for (case, spoil, why) in cases {
        let caller = Caller::new("files");
        caller.build("target/release", REPO, &Stub::default());
        spoil(&caller);
        let run = caller.package(&["9.9.9", "darwin-arm64"], stale, &[]);
        results.push(run.refused(case, why));
    }
    all_ok(results);
}

/// A manifest that does not say what it ships, or says it in a way that would
/// quietly mean something else, is refused before anything is built.
#[test]
fn a_manifest_that_does_not_say_what_it_ships_is_refused() {
    let cases: [(&str, &str, &str, &str); 8] = [
        ("no table", REPO, "", "has no [package.metadata.package-cli] table"),
        (
            "a misspelt key",
            REPO,
            "[package.metadata.package-cli]\nnames = { \"fs.example\" = 1 }\nlicences = [\"LICENSE\"]\n",
            "has licences, which is not names, licenses or caveats",
        ),
        (
            "no names",
            REPO,
            "[package.metadata.package-cli]\nnames = {}\nlicenses = [\"LICENSE\"]\n",
            "names is not a table of dotted name = man section",
        ),
        (
            "a dotted name written unquoted",
            REPO,
            "[package.metadata.package-cli]\nnames = { fs.example = 1 }\nlicenses = [\"LICENSE\"]\n",
            "a dotted name written unquoted is a nested table",
        ),
        (
            "a section that is not one",
            REPO,
            "[package.metadata.package-cli]\nnames = { \"fs.example\" = 0 }\nlicenses = [\"LICENSE\"]\n",
            "the man section 0",
        ),
        (
            "no licences",
            REPO,
            "[package.metadata.package-cli]\nnames = { \"fs.example\" = 1 }\nlicenses = []\n",
            "licenses is not a list of file names at the root",
        ),
        (
            "a CAVEATS outside the repository",
            REPO,
            "[package.metadata.package-cli]\nnames = { \"fs.example\" = 1 }\nlicenses = [\"LICENSE\"]\ncaveats = \"../CAVEATS\"\n",
            "caveats is '../CAVEATS', not a path inside the repository",
        ),
        (
            "a repository with no binary of its name",
            "rust-fs-renamed",
            TABLE,
            "has no [[bin]] named 'rust-fs-other'",
        ),
    ];
    let mut results = Vec::new();
    for (case, repo_name, table, why) in cases {
        let caller = Caller::with("manifest-refused", repo_name, table);
        if repo_name != REPO {
            // The repository is named one thing and the binary another.
            let manifest = caller.root.join("Cargo.toml");
            let text = fs::read_to_string(&manifest)
                .unwrap()
                .replace("example-org/rust-fs-renamed", "example-org/rust-fs-other");
            fs::write(&manifest, text).unwrap();
        }
        caller.build("target/release", REPO, &Stub::default());
        let run = caller.package(&["9.9.9", "darwin-arm64"], "", &[]);
        results.push(run.refused(case, why));
    }
    all_ok(results);
}

#[test]
fn the_arguments_are_checked() {
    let caller = Caller::new("args");
    caller.build("target/release", REPO, &Stub::default());
    let cases: [(&str, &[&str], &str); 5] = [
        ("a missing label", &["9.9.9", ""], "no LABEL"),
        ("a missing version", &["", "darwin-arm64"], "no VERSION"),
        (
            "one argument",
            &["9.9.9"],
            "usage: scripts/core.sh package-cli",
        ),
        (
            "four arguments",
            &["9.9.9", "a", "b", "c"],
            "usage: scripts/core.sh package-cli",
        ),
        ("a label with a /", &["9.9.9", "../x"], "cannot hold a /"),
    ];
    let mut results = Vec::new();
    for (case, args, why) in cases {
        let run = caller.package(args, "", &[]);
        results.push(run.refused(case, why));
    }
    all_ok(results);
}

/// A working directory that cannot be made is a failure too, after the
/// tarball's name is known: a previous run's tarball must not survive it. An
/// mktemp that fails stands in for an unusable TMPDIR, because macOS mktemp
/// falls back to the per-user directory when TMPDIR alone is unusable.
#[test]
fn a_working_directory_that_cannot_be_made_leaves_no_tarball() {
    let caller = Caller::new("mktemp");
    caller.build("target/release", REPO, &Stub::default());
    caller.write(
        "failing/mktemp",
        "#!/bin/sh\necho 'mktemp: cannot create directory' >&2\nexit 1\n",
    );
    let shim = caller.root.join("failing");
    fs::set_permissions(shim.join("mktemp"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = caller.package(
        &["9.9.9", "darwin-arm64"],
        "am-fs-example-9.9.9-darwin-arm64.tar.gz",
        &[("PATH", path)],
    );
    all_ok(vec![run.refused(
        "an unusable working directory",
        "could not make a working directory",
    )]);
}

/// The copies must not come back: family-check refuses a committed
/// scripts/package-cli.sh, and anything that runs one instead of core's.
#[test]
fn family_check_refuses_a_copy_of_package_cli() {
    let caller = Caller::new("family");
    // A caller keeps no bootstrap now (#212); family-check runs in place.
    fs::remove_file(caller.root.join("scripts/core.sh")).unwrap();
    let check = |caller: &Caller| {
        Command::new("bash")
            .current_dir(&caller.root)
            .arg(repo().join("scripts").join("family-check.sh"))
            .env("FS_CORE_CALLER", &caller.root)
            .output()
            .unwrap()
    };
    let clean = check(&caller);
    assert!(clean.status.success(), "{}", printed(&clean));

    caller.write("scripts/package-cli.sh", "echo mine\n");
    let copy = check(&caller);
    assert_eq!(copy.status.code(), Some(1), "{}", printed(&copy));
    assert!(
        String::from_utf8_lossy(&copy.stdout).contains("scripts/package-cli.sh is a copy"),
        "{}",
        printed(&copy)
    );
    fs::remove_file(caller.root.join("scripts/package-cli.sh")).unwrap();

    caller.write(
        ".github/workflows/ci.yml",
        "jobs:\n  cli:\n    steps:\n      # scripts/package-cli.sh is core's now\n      - run: tarball=\"$(../../scripts/package-cli.sh \"$v\" linux-x86_64)\"\n",
    );
    let calls = check(&caller);
    assert_eq!(calls.status.code(), Some(1), "{}", printed(&calls));
    let said = String::from_utf8_lossy(&calls.stdout);
    assert!(
        said.contains("../../scripts/package-cli.sh"),
        "{}",
        printed(&calls)
    );
    assert!(
        !said.contains("is core's now"),
        "a comment was counted as a call: {}",
        printed(&calls)
    );
}
