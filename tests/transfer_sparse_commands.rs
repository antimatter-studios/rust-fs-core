//! Command policies are graded by native file allocation and SHA-256 tools.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{self as t, *};
use fs_core::cli::{CliError, Json};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("core-sparse-command-{}-{tag}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn err(e: std::io::Error) -> CliError {
    CliError::failed(e.to_string())
}
fn open_for_times(path: &Path) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Directories need FILE_FLAG_BACKUP_SEMANTICS, and changing their
        // timestamps only needs FILE_WRITE_ATTRIBUTES access.
        fs::OpenOptions::new()
            .access_mode(0x0100)
            .share_mode(0x0001 | 0x0002 | 0x0004)
            .custom_flags(0x0200_0000)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}
struct Sink {
    root: PathBuf,
    staged: Option<(PathBuf, File)>,
    sparse: bool,
    writes: u64,
    fail: bool,
}
impl Sink {
    fn new(root: PathBuf, sparse: bool) -> Self {
        fs::create_dir(&root).unwrap();
        Self {
            root,
            staged: None,
            sparse,
            writes: 0,
            fail: false,
        }
    }
    fn local(&self, p: &Path) -> PathBuf {
        self.root.join(p.strip_prefix("/").unwrap_or(p))
    }
}
impl FileSink for Sink {
    fn supports_sparse(&self) -> bool {
        self.sparse
    }
    fn set_sparse_len(&mut self, len: u64) -> Result<(), CliError> {
        self.staged.as_mut().unwrap().1.set_len(len).map_err(err)
    }
    fn stage(&mut self, p: &Path, _: bool) -> Result<(), CliError> {
        let dest = self.local(p);
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(self.root.join("stage"))
            .map_err(err)?;
        self.staged = Some((dest, file));
        Ok(())
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        if self.fail {
            return Err(CliError::failed("injected write failure"));
        }
        let file = &mut self.staged.as_mut().unwrap().1;
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        file.write_all(bytes).map_err(err)?;
        self.writes += bytes.len() as u64;
        Ok(())
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        let file = &mut self.staged.as_mut().unwrap().1;
        file.seek(SeekFrom::Start(offset)).map_err(err)?;
        file.read(bytes).map_err(err)
    }
    fn persist(&mut self) -> Result<(), CliError> {
        self.staged.as_ref().unwrap().1.sync_all().map_err(err)
    }
    fn publish(&mut self, _: bool) -> Result<bool, CliError> {
        let (dest, file) = self.staged.take().unwrap();
        drop(file);
        fs::rename(self.root.join("stage"), dest).map_err(err)?;
        Ok(false)
    }
    fn abort(&mut self) -> Result<(), CliError> {
        self.staged = None;
        fs::remove_file(self.root.join("stage")).map_err(err)
    }
}
impl TreeSink for Sink {
    fn format_name(&self) -> &str {
        "examplefs"
    }
    fn validate_name(&mut self, _: &Path, name: &OsStr) -> Result<OsString, CliError> {
        Ok(name.into())
    }
    fn create_dir(&mut self, p: &Path) -> Result<bool, CliError> {
        fs::create_dir(self.local(p)).map_err(err)?;
        Ok(false)
    }
    fn set_times(
        &mut self,
        p: &Path,
        accessed: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> Result<(), CliError> {
        let mut times = fs::FileTimes::new();
        if let Some(time) = accessed {
            times = times.set_accessed(time);
        }
        if let Some(time) = modified {
            times = times.set_modified(time);
        }
        open_for_times(&self.local(p))
            .map_err(err)?
            .set_times(times)
            .map_err(err)
    }
}
#[cfg(unix)]
struct Source(File);
#[cfg(unix)]
impl FileSource for Source {
    fn size(&self) -> u64 {
        self.0.metadata().unwrap().len()
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        self.0.seek(SeekFrom::Start(offset)).map_err(err)?;
        self.0.read(bytes).map_err(err)
    }
}
#[cfg(unix)]
struct Tree;
#[cfg(unix)]
impl TreeSource for Tree {
    fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
        let m = fs::metadata(p).map_err(err)?;
        Ok(TreeMetadata {
            kind: if m.is_dir() {
                TreeKind::Directory
            } else {
                TreeKind::File
            },
            size: m.len(),
            inode: if m.is_dir() { 1 } else { 2 },
            accessed: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
        })
    }
    fn read_dir(&mut self, p: &Path) -> Result<Vec<TreeEntry>, CliError> {
        fs::read_dir(p)
            .map_err(err)?
            .map(|entry| {
                let e = entry.map_err(err)?;
                Ok(TreeEntry {
                    name: e.file_name(),
                    metadata: self.metadata(&e.path())?,
                })
            })
            .collect()
    }
    fn read_file_at(&mut self, p: &Path, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        FileSource::read_at(&mut Source(File::open(p).map_err(err)?), offset, bytes)
    }
}
fn import_matches(source: &Path) -> fs_core::cli::clap::ArgMatches {
    Command::new("import")
        .args(t::import_args_with_tree())
        .try_get_matches_from([
            OsStr::new("import"),
            source.as_os_str(),
            OsStr::new("/dest"),
            OsStr::new("--recursive"),
            OsStr::new("--preserve=sparse,times,links"),
        ])
        .unwrap()
}

#[test]
fn sparse_tree_refusals_name_every_file_and_the_format_before_staging() {
    let dir = Scratch::new("refusal");
    let input = dir.0.join("input");
    fs::create_dir(&input).unwrap();
    for name in ["a", "b"] {
        fs::write(input.join(name), [0; 17]).unwrap();
    }
    let m = import_matches(&input);
    let mut sink = Sink::new(dir.0.join("output"), false);
    let out = import_tree_with_sparse(
        &ImportRequest::from_matches(&m).unwrap(),
        Some(&mut sink),
        ImportTreeOptions::from_matches(&m).unwrap(),
        SparseOptions::from_matches(&m).unwrap(),
    )
    .unwrap();
    assert_ne!(out.code, 0);
    let report = out.report.unwrap();
    assert_eq!(report.get("refused"), Some(&Json::UInt(2)));
    let text = report.to_compact();
    for name in ["a", "b"] {
        assert!(text.contains(&format!("/dest/{name}")), "{text}");
        assert!(!sink.root.join("dest").join(name).exists());
    }
    assert_eq!(
        text.matches("sparse files not supported by examplefs")
            .count(),
        2
    );
    assert!(sink.staged.is_none());
    assert_eq!(sink.writes, 0);
}

#[test]
fn sparse_command_import_aborts_staging_on_write_failure() {
    let dir = Scratch::new("abort");
    let source = dir.0.join("source");
    fs::write(&source, vec![9; CHUNK + 1]).unwrap();
    let m = import_matches(&source);
    let mut sink = Sink::new(dir.0.join("output"), true);
    sink.fail = true;
    let error = import_with_sparse(
        &ImportRequest::from_matches(&m).unwrap(),
        ImportOptions { verify: true },
        SparseOptions::from_matches(&m).unwrap(),
        Some(&mut sink),
    )
    .unwrap_err();
    assert!(error.message.contains("injected write failure"));
    assert!(sink.staged.is_none());
    assert!(!sink.root.join("dest").exists());
    assert!(!sink.root.join("stage").exists());
}

#[cfg(unix)]
fn digest(p: &Path) -> String {
    let out = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(p)
        .output()
        .or_else(|_| std::process::Command::new("sha256sum").arg(p).output())
        .expect("install a host SHA-256 oracle");
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}

#[cfg(unix)]
#[test]
fn parsed_sparse_command_and_recursive_copies_match_host_holes_and_digest() {
    use std::os::unix::fs::MetadataExt;
    let dir = Scratch::new("oracle");
    let input = dir.0.join("input");
    fs::create_dir(&input).unwrap();
    let source = input.join("file");
    let mut file = File::create(&source).unwrap();
    file.write_all(&vec![19; CHUNK]).unwrap();
    file.seek(SeekFrom::Start((33 * CHUNK) as u64)).unwrap();
    file.write_all(&vec![37; CHUNK]).unwrap();
    file.set_len((66 * CHUNK + 17) as u64).unwrap();
    file.sync_all().unwrap();
    let expected = fs::metadata(&source).unwrap();
    assert!(
        expected.blocks() * 512 < expected.len(),
        "native oracle has no holes"
    );
    let hash = digest(&source);
    for recursive in [false, true] {
        let dest = dir.0.join(format!("export-{recursive}"));
        let m = Command::new("export")
            .args(export_args_with_times())
            .try_get_matches_from([
                OsStr::new("export"),
                if recursive {
                    input.as_os_str()
                } else {
                    source.as_os_str()
                },
                dest.as_os_str(),
                OsStr::new("--recursive"),
                OsStr::new("--verify"),
                OsStr::new("--preserve=sparse,times,links"),
            ])
            .unwrap();
        let request = ExportRequest::from_matches(&m).unwrap();
        let sparse = SparseOptions::from_matches(&m).unwrap();
        let out = if recursive {
            let mut log = Vec::new();
            let mut progress = Progress::new(&mut log, false, ProgressOptions { progress: true });
            let out = export_tree_with_sparse(
                &request,
                &mut Tree,
                TreeOptions::from_matches(&m).unwrap(),
                LinkOptions::from_matches(&m).unwrap(),
                TimeOptions::from_matches(&m).unwrap(),
                sparse,
                &mut progress,
            )
            .unwrap();
            drop(progress);
            let log = String::from_utf8(log).unwrap();
            let evidence: serde_json::Value =
                serde_json::from_str(log.lines().last().unwrap()).unwrap();
            assert_eq!(evidence["bytes"].as_u64(), Some(expected.len()));
            assert_eq!(evidence["complete"].as_bool(), Some(true));
            out
        } else {
            export_with_sparse(
                &request,
                &mut Source(File::open(&source).unwrap()),
                ExportOptions::from_matches(&m).unwrap(),
                sparse,
            )
            .unwrap()
        };
        assert_eq!(out.code, 0, "{:?}", out.report);
        let exported = if recursive { dest.join("file") } else { dest };
        let m = import_matches(if recursive { &input } else { &source });
        let mut sink = Sink::new(dir.0.join(format!("import-{recursive}")), true);
        let request = ImportRequest::from_matches(&m).unwrap();
        let sparse = SparseOptions::from_matches(&m).unwrap();
        let out = if recursive {
            import_tree_with_sparse(
                &request,
                Some(&mut sink),
                ImportTreeOptions::from_matches(&m).unwrap(),
                sparse,
            )
            .unwrap()
        } else {
            import_with_sparse(
                &request,
                ImportOptions { verify: true },
                sparse,
                Some(&mut sink),
            )
            .unwrap()
        };
        assert_eq!(out.code, 0, "{:?}", out.report);
        assert!(sink.writes < expected.len());
        for copied in [
            exported,
            sink.root.join(if recursive { "dest/file" } else { "dest" }),
        ] {
            let actual = fs::metadata(&copied).unwrap();
            assert_eq!(actual.len(), expected.len());
            assert!(actual.blocks() * 512 < actual.len(), "{actual:?}");
            assert_eq!(digest(&copied), hash);
        }
        assert!(out.report.unwrap().to_compact().contains(&hash));
    }
}
