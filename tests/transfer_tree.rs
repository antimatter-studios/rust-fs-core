//! Tree transfers checked against real host files and the platform SHA-256 tool.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command as Cmd;
use fs_core::cli::transfer::{
    self, ExportRequest, Output, TreeEntry, TreeKind, TreeMetadata, TreeOptions, TreeSource,
};
use fs_core::cli::{CliError, Json};
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Host {
    root: PathBuf,
    malicious: bool,
}
impl TreeSource for Host {
    fn metadata(&mut self, path: &Path) -> Result<TreeMetadata, CliError> {
        let m = fs::symlink_metadata(self.root.join(path))
            .map_err(|e| CliError::failed(e.to_string()))?;
        let mut id = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut id);
        Ok(TreeMetadata {
            kind: if m.is_dir() {
                TreeKind::Directory
            } else {
                TreeKind::File
            },
            size: m.len(),
            inode: id.finish(),
            accessed: m.accessed().ok(),
            modified: m.modified().ok(),
        })
    }
    fn read_dir(&mut self, path: &Path) -> Result<Vec<TreeEntry>, CliError> {
        let mut entries = Vec::new();
        for e in fs::read_dir(self.root.join(path)).map_err(|e| CliError::failed(e.to_string()))? {
            let name = e.unwrap().file_name();
            entries.push(TreeEntry {
                metadata: self.metadata(&path.join(&name))?,
                name,
            });
        }
        if self.malicious && path.as_os_str().is_empty() {
            for name in ["", ".", "..", "a/b", "a\\b", "nul\0name"] {
                entries.push(TreeEntry {
                    name: name.into(),
                    metadata: self.metadata(Path::new("good"))?,
                });
            }
            entries.push(TreeEntry {
                name: "special".into(),
                metadata: TreeMetadata {
                    kind: TreeKind::Other,
                    ..self.metadata(Path::new("good"))?
                },
            });
        }
        Ok(entries)
    }
    fn read_file_at(
        &mut self,
        path: &Path,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, CliError> {
        if path == Path::new("broken") {
            return Err(CliError::failed("unreadable source blocks"));
        }
        let mut f = fs::File::open(self.root.join(path)).unwrap();
        f.seek(SeekFrom::Start(offset)).unwrap();
        Ok(f.read(buf).unwrap())
    }
}
fn scratch(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("core-tree-{}-{tag}", std::process::id()));
    fs::create_dir(&p).unwrap();
    p
}
fn request(dest: &Path, replace: bool) -> ExportRequest {
    ExportRequest {
        path: OsString::new(),
        output: Output::File(dest.into()),
        replace,
    }
}
fn options() -> TreeOptions {
    TreeOptions {
        recursive: true,
        ..TreeOptions::default()
    }
}
fn count(report: &Json, status: &str) -> u64 {
    match report.get(status).unwrap() {
        Json::UInt(n) => *n,
        _ => panic!("count"),
    }
}
fn digest(p: &Path) -> String {
    let out = if cfg!(windows) {
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "(Get-FileHash -Algorithm SHA256 -LiteralPath '{}').Hash",
                p.display()
            ))
            .output()
            .expect("install PowerShell for SHA256 oracle")
    } else {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(p)
            .output()
            .expect("install Perl shasum for SHA256 oracle")
    };
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_ascii_lowercase()
}
#[test]
fn a_deep_tree_matches_host_digests_file_by_file() {
    let root = scratch("deep");
    let src = root.join("source");
    fs::create_dir(&src).unwrap();
    let mut relative = PathBuf::new();
    let mut files = Vec::new();
    for i in 0..24 {
        relative.push(format!("d{i}"));
        fs::create_dir(src.join(&relative)).unwrap();
        let f = relative.join("payload");
        fs::write(src.join(&f), vec![i as u8; 8193 + i]).unwrap();
        files.push(f);
    }
    let dest = root.join("dest");
    let result = transfer::export_tree(
        &request(&dest, false),
        &mut Host {
            root: src.clone(),
            malicious: false,
        },
        options(),
    )
    .unwrap();
    assert_eq!(result.code, 0);
    assert_eq!(count(result.report.as_ref().unwrap(), "completed"), 49);
    for f in files {
        assert_eq!(digest(&src.join(&f)), digest(&dest.join(&f)));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unsafe_names_and_failed_files_leave_completed_siblings_and_a_nonzero_report() {
    let root = scratch("partial");
    let src = root.join("source");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("good"), b"good").unwrap();
    fs::write(src.join("broken"), b"bad").unwrap();
    let dest = root.join("dest");
    let result = transfer::export_tree(
        &request(&dest, false),
        &mut Host {
            root: src,
            malicious: true,
        },
        options(),
    )
    .unwrap();
    assert_eq!(result.code, 1);
    let report = result.report.unwrap();
    assert_eq!(count(&report, "completed"), 2);
    assert_eq!(count(&report, "refused"), 6);
    assert_eq!(count(&report, "failed"), 1);
    assert_eq!(count(&report, "skipped"), 1);
    let text = report.to_compact();
    for reason in [
        "unreadable source blocks",
        "unsafe entry name",
        "unsupported entry type",
        "merged_directories",
    ] {
        assert!(text.contains(reason), "{text}");
    }
    assert_eq!(fs::read(dest.join("good")).unwrap(), b"good");
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
    assert!(!root.join("good").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn existing_directories_merge_but_files_require_replace() {
    let root = scratch("merge");
    let src = root.join("source");
    let dest = root.join("dest");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::create_dir(src.join("sub")).unwrap();
    fs::create_dir(dest.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"new").unwrap();
    fs::write(dest.join("sub/file"), b"old").unwrap();
    fs::write(dest.join("keep"), b"keep").unwrap();
    let mut host = Host {
        root: src,
        malicious: false,
    };
    let refused = transfer::export_tree(&request(&dest, false), &mut host, options()).unwrap();
    assert_eq!(refused.code, 1);
    assert_eq!(count(refused.report.as_ref().unwrap(), "refused"), 1);
    assert_eq!(fs::read(dest.join("sub/file")).unwrap(), b"old");
    let replaced = transfer::export_tree(&request(&dest, true), &mut host, options()).unwrap();
    assert_eq!(replaced.code, 0);
    assert_eq!(
        count(replaced.report.as_ref().unwrap(), "merged_directories"),
        2
    );
    assert_eq!(fs::read(dest.join("sub/file")).unwrap(), b"new");
    assert_eq!(fs::read(dest.join("keep")).unwrap(), b"keep");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn directories_need_recursive_and_depth_is_bounded() {
    let root = scratch("depth");
    let src = root.join("source");
    fs::create_dir(&src).unwrap();
    fs::create_dir(src.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"x").unwrap();
    let dest = root.join("dest");
    let mut host = Host {
        root: src,
        malicious: false,
    };
    let e = transfer::export_tree(&request(&dest, false), &mut host, TreeOptions::default())
        .unwrap_err();
    assert!(e.message.contains("--recursive"));
    assert!(!dest.exists());
    let result = transfer::export_tree(
        &request(&dest, false),
        &mut host,
        TreeOptions {
            max_depth: 1,
            ..options()
        },
    )
    .unwrap();
    assert_eq!(result.code, 1);
    assert_eq!(count(result.report.as_ref().unwrap(), "failed"), 1);
    assert!(!dest.join("sub/file").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn tree_arguments_document_defaults_and_merge_policy() {
    let cmd = || Cmd::new("export").args(transfer::export_args_with_tree());
    let mut help = cmd();
    let help = help.render_long_help().to_string();
    for word in ["--recursive", "--max-depth", "64", "merged"] {
        assert!(help.contains(word), "{help}");
    }
    let m = cmd()
        .try_get_matches_from([
            "export",
            "/",
            "out",
            "--recursive",
            "--max-depth",
            "3",
            "--verify",
        ])
        .unwrap();
    let o = TreeOptions::from_matches(&m).unwrap();
    assert!(o.recursive && o.verify);
    assert_eq!(o.max_depth, 3);
    let m = cmd().try_get_matches_from(["export", "/", "out"]).unwrap();
    assert_eq!(
        TreeOptions::from_matches(&m).unwrap(),
        TreeOptions::default()
    );
}
#[cfg(unix)]
#[test]
fn an_existing_destination_symlink_cannot_redirect_the_tree() {
    let root = scratch("symlink");
    let src = root.join("source");
    let dest = root.join("dest");
    let outside = root.join("outside");
    for p in [&src, &dest, &outside] {
        fs::create_dir(p).unwrap();
    }
    fs::create_dir(src.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"x").unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("sub")).unwrap();
    let result = transfer::export_tree(
        &request(&dest, true),
        &mut Host {
            root: src,
            malicious: false,
        },
        options(),
    )
    .unwrap();
    assert_eq!(result.code, 1);
    assert!(!outside.join("file").exists());
    fs::remove_dir_all(root).unwrap();
}
