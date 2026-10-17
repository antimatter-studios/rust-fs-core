//! Host files and the platform SHA-256 tool independently grade tree imports.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{self, FileSink, ImportRequest, ImportTreeOptions, Input, TreeSink};
use fs_core::cli::{CliError, Json, Outcome};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct Sink {
    files: BTreeMap<PathBuf, Vec<u8>>,
    dirs: Vec<PathBuf>,
    staged: Option<(PathBuf, Vec<u8>)>,
    checked: Vec<PathBuf>,
    fail: Option<PathBuf>,
}
impl FileSink for Sink {
    fn stage(&mut self, path: &Path, _: bool) -> Result<(), CliError> {
        assert!(self.checked.contains(&path.to_path_buf()));
        assert!(!self.files.contains_key(path));
        self.staged = Some((path.into(), Vec::new()));
        Ok(())
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        let (path, data) = self.staged.as_mut().unwrap();
        if self.fail.as_ref() == Some(path) {
            return Err(CliError::failed("sink full"));
        }
        assert_eq!(offset as usize, data.len());
        data.extend_from_slice(bytes);
        Ok(())
    }
    fn persist(&mut self) -> Result<(), CliError> {
        Ok(())
    }
    fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize, CliError> {
        unreachable!()
    }
    fn publish(&mut self, _: bool) -> Result<bool, CliError> {
        let (path, data) = self.staged.take().unwrap();
        self.files.insert(path, data);
        Ok(false)
    }
    fn abort(&mut self) -> Result<(), CliError> {
        self.staged = None;
        Ok(())
    }
}
impl TreeSink for Sink {
    fn validate_name(&mut self, parent: &Path, name: &OsStr) -> Result<OsString, CliError> {
        let text = name.to_str().ok_or_else(|| CliError::refused("encoding"))?;
        if text == "reserved" {
            return Err(CliError::refused("reserved name"));
        }
        let key = text.to_ascii_lowercase();
        for existing in self.files.keys().chain(self.dirs.iter()) {
            if existing.parent() == Some(parent)
                && existing.file_name() != Some(name)
                && existing
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    == key
            {
                return Err(CliError::refused("existing case collision"));
            }
        }
        self.checked.push(parent.join(name));
        Ok(key.into())
    }
    fn create_dir(&mut self, path: &Path) -> Result<bool, CliError> {
        assert!(self.checked.contains(&path.to_path_buf()));
        let merged = self.dirs.contains(&path.to_path_buf());
        if !merged {
            self.dirs.push(path.into());
        }
        Ok(merged)
    }
}
struct Local(PathBuf);
impl Local {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("core-import-tree-{}-{tag}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn run(&self, sink: &mut Sink, max_depth: usize) -> Outcome {
        transfer::import_tree(
            &ImportRequest {
                input: Input::File(self.0.clone()),
                path: "/dest".into(),
                replace: false,
            },
            Some(sink),
            ImportTreeOptions {
                recursive: true,
                max_depth,
                ..ImportTreeOptions::default()
            },
        )
        .unwrap()
    }
}
impl Local {
    fn run_capable(&self, sink: &mut Capable) -> Outcome {
        transfer::import_tree(
            &ImportRequest {
                input: Input::File(self.0.clone()),
                path: "/dest".into(),
                replace: false,
            },
            Some(sink),
            ImportTreeOptions {
                recursive: true,
                ..ImportTreeOptions::default()
            },
        )
        .unwrap()
    }
}
impl Drop for Local {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn count(out: &Outcome, key: &str) -> u64 {
    match out.report.as_ref().unwrap().get(key).unwrap() {
        Json::UInt(n) => *n,
        _ => panic!("count"),
    }
}
fn digest(path: &Path) -> String {
    let out = if cfg!(windows) {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
                path.display()
            ))
            .output()
    } else {
        std::process::Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
    }
    .expect("install the platform SHA-256 oracle");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_ascii_lowercase()
}
#[test]
fn a_deep_tree_imports_host_bytes_and_independent_digests() {
    let local = Local::new("deep");
    let mut relative = PathBuf::new();
    for i in 0..12 {
        relative.push(format!("d{i}"));
        fs::create_dir(local.0.join(&relative)).unwrap();
        fs::write(local.0.join(relative.join("payload")), [i as u8; 8193]).unwrap();
    }
    let mut sink = Sink::default();
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 0);
    assert_eq!(count(&out, "completed"), 25);
    let report = out.report.unwrap().to_compact();
    for (path, data) in &sink.files {
        let host = local.0.join(path.strip_prefix("/dest").unwrap());
        assert_eq!(*data, fs::read(&host).unwrap());
        assert!(report.contains(&digest(&host)));
    }
}
#[test]
fn reserved_names_and_existing_case_collisions_refuse_while_siblings_complete() {
    let local = Local::new("refused");
    for name in ["reserved", "Taken", "good"] {
        fs::write(local.0.join(name), b"abc").unwrap();
    }
    let mut sink = Sink::default();
    sink.files.insert("/dest/taken".into(), b"old".to_vec());
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "refused"), 2);
    assert_eq!(count(&out, "completed"), 2);
    assert_eq!(sink.files[Path::new("/dest/taken")], b"old");
    assert_eq!(sink.files[Path::new("/dest/good")], b"abc");
    assert!(!sink.files.contains_key(Path::new("/dest/reserved")));
}
#[test]
fn failed_files_abort_staging_and_leave_a_partial_report() {
    let local = Local::new("failed");
    for name in ["broken", "good"] {
        fs::write(local.0.join(name), b"abc").unwrap();
    }
    let mut sink = Sink {
        fail: Some("/dest/broken".into()),
        ..Sink::default()
    };
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "failed"), 1);
    assert_eq!(count(&out, "completed"), 2);
    assert!(sink.staged.is_none());
    assert_eq!(sink.files.len(), 1);
}
#[test]
fn depth_limits_fail_without_creating_deeper_destinations() {
    let local = Local::new("depth");
    fs::create_dir(local.0.join("deep")).unwrap();
    let mut sink = Sink::default();
    let out = local.run(&mut sink, 0);
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "failed"), 1);
    assert_eq!(sink.dirs.len(), 1);
}
#[test]
fn recursive_arguments_are_additive_and_stdin_is_refused() {
    let m = Command::new("import")
        .args(transfer::import_args_with_tree())
        .try_get_matches_from(["import", "--recursive", "-", "/dest"])
        .unwrap();
    let request = ImportRequest::from_matches(&m).unwrap();
    let options = ImportTreeOptions::from_matches(&m).unwrap();
    assert!(options.recursive);
    assert_eq!(
        transfer::import_tree(&request, Some(&mut Sink::default()), options)
            .unwrap_err()
            .code,
        2
    );
}
#[test]
fn directories_require_recursive_and_missing_sinks_refuse_before_io() {
    let local = Local::new("recursive");
    let request = ImportRequest {
        input: Input::File(local.0.clone()),
        path: "/dest".into(),
        replace: false,
    };
    assert_eq!(
        transfer::import_tree(&request, None, ImportTreeOptions::default())
            .unwrap_err()
            .code,
        3
    );
    assert_eq!(
        transfer::import_tree(
            &request,
            Some(&mut Sink::default()),
            ImportTreeOptions::default()
        )
        .unwrap_err()
        .code,
        2
    );
}
#[cfg(unix)]
#[test]
fn symlinks_are_refused_without_following_their_targets() {
    let local = Local::new("symlink");
    std::os::unix::fs::symlink(".", local.0.join("loop")).unwrap();
    std::os::unix::fs::symlink("missing", local.0.join("dangling")).unwrap();
    let mut sink = Sink::default();
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "refused"), 2);
    assert_eq!(sink.dirs.len(), 1);
    assert!(sink.files.is_empty());
    let request = ImportRequest {
        input: Input::File(local.0.clone()),
        path: "/dest".into(),
        replace: false,
    };
    let options = ImportTreeOptions {
        recursive: true,
        preserve_links: true,
        ..ImportTreeOptions::default()
    };
    let out = transfer::import_tree(&request, Some(&mut sink), options).unwrap();
    assert_eq!(count(&out, "refused"), 2);
    assert!(out
        .report
        .unwrap()
        .to_compact()
        .contains("symbolic links not supported by this sink"));
}

#[cfg(target_os = "linux")]
#[test]
fn distinct_source_spellings_share_the_sinks_case_key() {
    let local = Local::new("sibling-case");
    fs::write(local.0.join("Name"), b"first").unwrap();
    fs::write(local.0.join("name"), b"second").unwrap();
    let mut sink = Sink::default();
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "refused"), 1);
    assert_eq!(count(&out, "completed"), 2);
    assert_eq!(sink.files.len(), 1);
    assert_eq!(sink.files[Path::new("/dest/Name")], b"first");
}

#[test]
fn exact_existing_directories_merge_without_changing_unrelated_files() {
    let local = Local::new("merge");
    fs::write(local.0.join("good"), b"abc").unwrap();
    let mut sink = Sink::default();
    sink.dirs.push("/dest".into());
    sink.files.insert("/dest/unrelated".into(), b"old".to_vec());
    let out = local.run(&mut sink, 64);
    assert_eq!(out.code, 0);
    assert_eq!(count(&out, "merged_directories"), 1);
    assert_eq!(sink.files[Path::new("/dest/unrelated")], b"old");
}

#[test]
fn a_regular_file_uses_the_checked_transaction_without_recursive() {
    let local = Local::new("file");
    let file = local.0.join("input");
    fs::write(&file, b"abc").unwrap();
    let request = ImportRequest {
        input: Input::File(file),
        path: "/dest".into(),
        replace: false,
    };
    let mut sink = Sink::default();
    let out =
        transfer::import_tree(&request, Some(&mut sink), ImportTreeOptions::default()).unwrap();
    assert_eq!(out.code, 0);
    assert_eq!(count(&out, "completed"), 1);
    assert_eq!(sink.files[Path::new("/dest")], b"abc");
}

/// The in-memory sink with every optional metadata capability switched on.
#[derive(Default)]
struct Capable {
    inner: Sink,
    hard_links: BTreeMap<PathBuf, PathBuf>,
    symbolic_links: BTreeMap<PathBuf, PathBuf>,
    times: Vec<(PathBuf, Option<SystemTime>, Option<SystemTime>)>,
}
impl FileSink for Capable {
    fn stage(&mut self, path: &Path, replace: bool) -> Result<(), CliError> {
        self.inner.stage(path, replace)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        self.inner.write_at(offset, bytes)
    }
    fn persist(&mut self) -> Result<(), CliError> {
        self.inner.persist()
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        self.inner.read_at(offset, bytes)
    }
    fn publish(&mut self, replace: bool) -> Result<bool, CliError> {
        self.inner.publish(replace)
    }
    fn abort(&mut self) -> Result<(), CliError> {
        self.inner.abort()
    }
}
impl TreeSink for Capable {
    fn format_name(&self) -> &str {
        "memfs"
    }
    fn validate_name(&mut self, parent: &Path, name: &OsStr) -> Result<OsString, CliError> {
        self.inner.validate_name(parent, name)
    }
    fn create_dir(&mut self, path: &Path) -> Result<bool, CliError> {
        self.inner.create_dir(path)
    }
    fn create_symbolic_link(&mut self, path: &Path, target: &Path) -> Result<(), CliError> {
        assert!(self
            .symbolic_links
            .insert(path.into(), target.into())
            .is_none());
        Ok(())
    }
    fn create_hard_link(&mut self, path: &Path, existing: &Path) -> Result<(), CliError> {
        assert!(
            self.inner.files.contains_key(existing),
            "link to a published file"
        );
        assert!(self
            .hard_links
            .insert(path.into(), existing.into())
            .is_none());
        Ok(())
    }
    fn set_times(
        &mut self,
        path: &Path,
        accessed: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> Result<(), CliError> {
        self.times.push((path.into(), accessed, modified));
        Ok(())
    }
}
/// A tree holding a file, a second hard link to it, a symbolic link and a
/// directory, every time pinned to a value the host filesystem reports back.
#[cfg(unix)]
fn metadata_tree(tag: &str) -> (Local, SystemTime) {
    let local = Local::new(tag);
    let pinned = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::write(local.0.join("a"), b"shared bytes").unwrap();
    fs::hard_link(local.0.join("a"), local.0.join("b")).unwrap();
    std::os::unix::fs::symlink("a", local.0.join("c")).unwrap();
    fs::create_dir(local.0.join("d")).unwrap();
    fs::write(local.0.join("d/x"), b"inner").unwrap();
    for name in ["a", "d/x", "d", ""] {
        let times = fs::FileTimes::new()
            .set_accessed(pinned)
            .set_modified(pinned);
        fs::File::open(local.0.join(name))
            .unwrap()
            .set_times(times)
            .unwrap();
    }
    assert_eq!(
        fs::metadata(local.0.join("d")).unwrap().modified().unwrap(),
        pinned
    );
    (local, pinned)
}
#[cfg(unix)]
fn preserve_everything() -> ImportTreeOptions {
    let m = Command::new("import")
        .args(transfer::import_args_with_tree())
        .try_get_matches_from([
            "import",
            "--recursive",
            "--preserve=times,links",
            "src",
            "/dest",
        ])
        .unwrap();
    let options = ImportTreeOptions::from_matches(&m).unwrap();
    assert!(options.preserve_links && options.preserve_times);
    options
}
#[cfg(unix)]
fn status_of(out: &Outcome, destination: &str) -> (String, String) {
    let report = out.report.as_ref().unwrap().to_compact();
    let at = report
        .find(&format!("\"destination\":\"{destination}\""))
        .unwrap_or_else(|| panic!("{destination} missing from {report}"));
    let item = &report[at..report[at..].find('}').map_or(report.len(), |e| at + e)];
    let field = |key: &str| {
        let start = item.find(&format!("\"{key}\":\"")).unwrap() + key.len() + 4;
        item[start..start + item[start..].find('"').unwrap()].to_string()
    };
    (field("status"), field("reason"))
}
#[cfg(unix)]
#[test]
fn capable_sinks_receive_times_hard_links_and_symbolic_links() {
    let (local, pinned) = metadata_tree("capable");
    let mut sink = Capable::default();
    let request = ImportRequest {
        input: Input::File(local.0.clone()),
        path: "/dest".into(),
        replace: false,
    };
    let out = transfer::import_tree(&request, Some(&mut sink), preserve_everything()).unwrap();
    assert_eq!(out.code, 0, "{}", out.report.as_ref().unwrap().to_compact());
    assert_eq!(count(&out, "completed"), 6);
    assert_eq!(
        sink.inner.files[Path::new("/dest/a")],
        fs::read(local.0.join("a")).unwrap()
    );
    assert!(!sink.inner.files.contains_key(Path::new("/dest/b")));
    assert_eq!(sink.hard_links[Path::new("/dest/b")], Path::new("/dest/a"));
    assert_eq!(
        sink.symbolic_links[Path::new("/dest/c")],
        fs::read_link(local.0.join("c")).unwrap()
    );
    let set = |path: &str| {
        sink.times
            .iter()
            .position(|(p, _, _)| p == Path::new(path))
            .unwrap_or_else(|| panic!("no times set on {path}"))
    };
    for path in ["/dest", "/dest/a", "/dest/d", "/dest/d/x"] {
        let (_, accessed, modified) = &sink.times[set(path)];
        assert_eq!(
            (*accessed, *modified),
            (Some(pinned), Some(pinned)),
            "{path}"
        );
    }
    set("/dest/c");
    assert!(
        set("/dest/d") > set("/dest/d/x"),
        "directory times follow children"
    );
    assert!(set("/dest") > set("/dest/d"), "root times come last");
}
#[cfg(unix)]
#[test]
fn sinks_lacking_capabilities_refuse_and_name_every_affected_item() {
    let (local, _) = metadata_tree("incapable");
    let mut sink = Sink::default();
    let request = ImportRequest {
        input: Input::File(local.0.clone()),
        path: "/dest".into(),
        replace: false,
    };
    let out = transfer::import_tree(&request, Some(&mut sink), preserve_everything()).unwrap();
    assert_eq!(out.code, 1);
    assert_eq!(count(&out, "completed"), 0);
    assert_eq!(count(&out, "refused"), 6);
    let times = "times not supported by this sink".to_string();
    let refused = "refused".to_string();
    for path in ["/dest", "/dest/a", "/dest/d", "/dest/d/x"] {
        assert_eq!(
            status_of(&out, path),
            (refused.clone(), times.clone()),
            "{path}"
        );
    }
    let hard = "hard links not supported by this sink".to_string();
    assert_eq!(status_of(&out, "/dest/b"), (refused.clone(), hard));
    let symbolic = "symbolic links not supported by this sink".to_string();
    assert_eq!(status_of(&out, "/dest/c"), (refused, symbolic));
    assert_eq!(
        sink.files.keys().collect::<Vec<_>>(),
        [Path::new("/dest/a"), Path::new("/dest/d/x")]
    );
}
#[cfg(unix)]
#[test]
fn without_preserve_hard_links_copy_and_no_times_are_set() {
    let (local, _) = metadata_tree("plain");
    fs::remove_file(local.0.join("c")).unwrap();
    let mut sink = Capable::default();
    let out = local.run_capable(&mut sink);
    assert_eq!(out.code, 0);
    assert!(sink.times.is_empty() && sink.hard_links.is_empty());
    assert_eq!(sink.inner.files[Path::new("/dest/b")], b"shared bytes");
}
