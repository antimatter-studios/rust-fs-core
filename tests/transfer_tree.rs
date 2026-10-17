//! Tree transfers checked against real host files and the platform SHA-256 tool.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command as Cmd;
use fs_core::cli::transfer::{
    self, ExportRequest, Output, TreeEntry, TreeKind, TreeMetadata, TreeOptions, TreeSource,
};
use fs_core::cli::{CliError, Json, Outcome};
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
            } else if m.is_file() {
                TreeKind::File
            } else {
                TreeKind::Other
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
            let mut names = vec!["", ".", "..", "a/b", "a\\b", "nul\0name"];
            if cfg!(windows) {
                names.extend([".. ", "file.", "file:stream"]);
            }
            for name in names {
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
fn scratch(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let p = std::env::temp_dir().join(format!("core-tree-{}-{tag}", std::process::id()));
    let src = p.join("source");
    fs::create_dir_all(&src).unwrap();
    let dest = p.join("dest");
    (p, src, dest)
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
        verify: true,
        ..TreeOptions::default()
    }
}
fn run(src: &Path, dest: &Path, replace: bool) -> Outcome {
    let mut host = Host {
        root: src.into(),
        malicious: false,
    };
    transfer::export_tree(&request(dest, replace), &mut host, options()).unwrap()
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
    let (root, src, dest) = scratch("deep");
    let mut relative = PathBuf::new();
    let mut files = Vec::new();
    for i in 0..24 {
        relative.push(format!("d{i}"));
        fs::create_dir(src.join(&relative)).unwrap();
        let f = relative.join("payload");
        fs::write(src.join(&f), vec![i as u8; 8193 + i]).unwrap();
        files.push(f);
    }
    let result = run(&src, &dest, false);
    assert_eq!(result.code, 0);
    assert_eq!(count(result.report.as_ref().unwrap(), "completed"), 49);
    for f in files {
        assert_eq!(digest(&src.join(&f)), digest(&dest.join(&f)));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unsafe_names_and_failed_files_leave_completed_siblings_and_a_nonzero_report() {
    let (root, src, dest) = scratch("partial");
    fs::write(src.join("good"), b"good").unwrap();
    fs::write(src.join("broken"), b"bad").unwrap();
    let mut host = Host {
        root: src,
        malicious: true,
    };
    let result = transfer::export_tree(&request(&dest, false), &mut host, options()).unwrap();
    assert_eq!(result.code, 1);
    let report = result.report.unwrap();
    assert_eq!(count(&report, "completed"), 2);
    assert_eq!(count(&report, "refused"), if cfg!(windows) { 9 } else { 6 });
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
    let (root, src, dest) = scratch("merge");
    fs::create_dir(&dest).unwrap();
    fs::create_dir(src.join("sub")).unwrap();
    fs::create_dir(dest.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"new").unwrap();
    fs::write(dest.join("sub/file"), b"old").unwrap();
    fs::write(dest.join("keep"), b"keep").unwrap();
    let refused = run(&src, &dest, false);
    assert_eq!(refused.code, 1);
    assert_eq!(count(refused.report.as_ref().unwrap(), "refused"), 1);
    assert_eq!(fs::read(dest.join("sub/file")).unwrap(), b"old");
    let replaced = run(&src, &dest, true);
    assert_eq!(replaced.code, 0);
    let report = replaced.report.unwrap();
    assert_eq!(count(&report, "merged_directories"), 2);
    assert_eq!(fs::read(dest.join("sub/file")).unwrap(), b"new");
    assert_eq!(fs::read(dest.join("keep")).unwrap(), b"keep");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn directories_need_recursive_and_depth_is_bounded() {
    let (root, src, dest) = scratch("depth");
    fs::create_dir(src.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"x").unwrap();
    let mut host = Host {
        root: src,
        malicious: false,
    };
    let e = transfer::export_tree(&request(&dest, false), &mut host, TreeOptions::default())
        .unwrap_err();
    assert!(e.message.contains("--recursive"));
    assert!(!dest.exists());
    let bounded = TreeOptions {
        max_depth: 1,
        ..options()
    };
    let result = transfer::export_tree(&request(&dest, false), &mut host, bounded).unwrap();
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
    let argv = "export / out --recursive --max-depth 3 --verify";
    let m = cmd().try_get_matches_from(argv.split_whitespace()).unwrap();
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
    let (root, src, dest) = scratch("symlink");
    let outside = root.join("outside");
    for p in [&dest, &outside] {
        fs::create_dir(p).unwrap();
    }
    fs::create_dir(src.join("sub")).unwrap();
    fs::write(src.join("sub/file"), b"x").unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("sub")).unwrap();
    let result = run(&src, &dest, true);
    assert_eq!(result.code, 1);
    assert!(!outside.join("file").exists());
    fs::remove_dir_all(root).unwrap();
}

// An ordered driver fixture can expose both spellings even when the host
// holding the test checkout cannot. Destination lookup remains the OS oracle.
struct NamedTree {
    entries: Vec<(PathBuf, TreeKind)>,
    reads: Vec<PathBuf>,
    listings: Vec<PathBuf>,
}
impl NamedTree {
    fn new(entries: &[(&str, TreeKind)]) -> Self {
        Self {
            entries: entries.iter().map(|(p, k)| (p.into(), *k)).collect(),
            reads: Vec::new(),
            listings: Vec::new(),
        }
    }
    fn export(&mut self, dest: &Path, replace: bool) -> Outcome {
        transfer::export_tree(&request(dest, replace), self, options()).unwrap()
    }
}
impl TreeSource for NamedTree {
    fn metadata(&mut self, path: &Path) -> Result<TreeMetadata, CliError> {
        let (kind, inode) = if path.as_os_str().is_empty() {
            (TreeKind::Directory, 0)
        } else {
            let i = self.entries.iter().position(|(p, _)| p == path).unwrap();
            (self.entries[i].1, i as u64 + 1)
        };
        Ok(TreeMetadata {
            kind,
            size: path.to_str().unwrap().len() as u64,
            inode,
            accessed: None,
            modified: None,
        })
    }
    fn read_dir(&mut self, path: &Path) -> Result<Vec<TreeEntry>, CliError> {
        self.listings.push(path.into());
        let paths: Vec<_> = self
            .entries
            .iter()
            .filter(|(p, _)| p.parent() == Some(path))
            .map(|(p, _)| p.clone())
            .collect();
        paths
            .into_iter()
            .map(|p| {
                Ok(TreeEntry {
                    name: p.file_name().unwrap().into(),
                    metadata: self.metadata(&p)?,
                })
            })
            .collect()
    }
    fn read_file_at(
        &mut self,
        path: &Path,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, CliError> {
        self.reads.push(path.into());
        let portable = path.to_str().unwrap().replace('\\', "/");
        let data = portable.as_bytes();
        let data = &data[offset as usize..];
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
}
fn folds_case(dest: &Path) -> bool {
    fs::create_dir_all(dest).unwrap();
    let name = dest.join("CaseOracle");
    fs::write(&name, b"oracle").unwrap();
    let folds = dest.join("cASEoRACLE").try_exists().unwrap();
    if folds {
        assert_eq!(fs::read(dest.join("cASEoRACLE")).unwrap(), b"oracle");
    }
    fs::remove_file(name).unwrap();
    folds
}
fn check_case_pair(dest: &Path, folding: bool, replace: bool) {
    assert_eq!(folds_case(dest), folding, "fixture has wrong case behavior");
    let mut tree = NamedTree::new(&[("README", TreeKind::File), ("readme", TreeKind::File)]);
    let out = tree.export(dest, replace);
    let report = out.report.unwrap();
    assert_eq!(out.code, u8::from(folding), "{}", report.to_compact());
    assert_eq!(count(&report, "refused"), u64::from(folding));
    assert_eq!(count(&report, "failed"), 0);
    assert_eq!(fs::read(dest.join("README")).unwrap(), b"README");
    assert_eq!(
        fs::read_dir(dest).unwrap().count(),
        if folding { 1 } else { 2 }
    );
    if folding {
        assert!(report.to_compact().contains("collides with README"));
        assert_eq!(tree.reads, [PathBuf::from("README")]);
    } else {
        assert_eq!(fs::read(dest.join("readme")).unwrap(), b"readme");
    }
}
#[test]
fn case_variants_follow_the_destination_filesystem_with_and_without_replace() {
    let (root, _, dest) = scratch("case-pair");
    let folding = folds_case(&dest);
    for replace in [false, true] {
        check_case_pair(&dest.join(format!("replace-{replace}")), folding, replace);
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn every_later_collision_names_the_first_source_entry() {
    let (root, _, dest) = scratch("case-many");
    let folding = folds_case(&dest);
    let mut tree = NamedTree::new(&[
        ("README", TreeKind::File),
        ("readme", TreeKind::File),
        ("ReadMe", TreeKind::File),
        ("sibling", TreeKind::File),
    ]);
    let out = tree.export(&dest, true);
    let report = out.report.unwrap();
    assert_eq!(count(&report, "refused"), if folding { 2 } else { 0 });
    assert_eq!(fs::read(dest.join("README")).unwrap(), b"README");
    assert_eq!(fs::read(dest.join("sibling")).unwrap(), b"sibling");
    if folding {
        assert_eq!(
            report.to_compact().matches("collides with README").count(),
            2
        );
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn colliding_directories_are_refused_before_their_children_are_listed() {
    let (root, _, dest) = scratch("case-dirs");
    let folding = folds_case(&dest);
    let mut tree = NamedTree::new(&[
        ("DIR", TreeKind::Directory),
        ("dir", TreeKind::Directory),
        ("DIR/first", TreeKind::File),
        ("dir/second", TreeKind::File),
    ]);
    let out = tree.export(&dest, true);
    assert_eq!(
        count(out.report.as_ref().unwrap(), "refused"),
        u64::from(folding)
    );
    assert_eq!(fs::read(dest.join("DIR/first")).unwrap(), b"DIR/first");
    assert_eq!(tree.listings.contains(&PathBuf::from("dir")), !folding);
    assert_eq!(dest.join("dir/second").exists(), !folding);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn collisions_are_scoped_to_each_destination_directory() {
    let (root, _, dest) = scratch("case-scope");
    let mut tree = NamedTree::new(&[
        ("one", TreeKind::Directory),
        ("two", TreeKind::Directory),
        ("one/README", TreeKind::File),
        ("two/readme", TreeKind::File),
    ]);
    assert_eq!(tree.export(&dest, true).code, 0);
    assert_eq!(fs::read(dest.join("one/README")).unwrap(), b"one/README");
    assert_eq!(fs::read(dest.join("two/readme")).unwrap(), b"two/readme");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn duplicate_source_names_are_refused_even_on_case_sensitive_destinations() {
    let (root, _, dest) = scratch("case-duplicate");
    let mut tree = NamedTree::new(&[("same", TreeKind::File), ("same", TreeKind::File)]);
    let out = tree.export(&dest, true);
    let report = out.report.unwrap();
    assert_eq!(out.code, 1);
    assert_eq!(count(&report, "refused"), 1);
    assert!(report.to_compact().contains("collides with same"));
    assert_eq!(tree.reads, [PathBuf::from("same")]);
    assert_eq!(fs::read(dest.join("same")).unwrap(), b"same");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn mounted_case_folding_and_sensitive_images_decide_collision_policy() {
    struct Image {
        root: PathBuf,
        mount: PathBuf,
    }
    impl Drop for Image {
        fn drop(&mut self) {
            let out = Command::new("hdiutil")
                .arg("detach")
                .arg(&self.mount)
                .arg("-force")
                .output()
                .expect("hdiutil detach is required");
            if !std::thread::panicking() {
                assert!(out.status.success(), "detach failed: {out:?}");
            }
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    for (filesystem, folding) in [("HFS+", true), ("Case-sensitive HFS+", false)] {
        let (root, _, _) = scratch(if folding {
            "hfs-folding"
        } else {
            "hfs-sensitive"
        });
        let image = root.join("oracle.sparseimage");
        let mount = root.join("mount");
        fs::create_dir(&mount).unwrap();
        let out = Command::new("hdiutil")
            .args([
                "create",
                "-size",
                "32m",
                "-type",
                "SPARSE",
                "-fs",
                filesystem,
                "-volname",
                "core-case-oracle",
            ])
            .arg(&image)
            .output()
            .expect("hdiutil create is required for the case oracle");
        assert!(out.status.success(), "create failed: {out:?}");
        let out = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-noautoopen", "-mountpoint"])
            .arg(&mount)
            .arg(&image)
            .output()
            .expect("hdiutil attach is required for the case oracle");
        let volume = Image { root, mount };
        assert!(out.status.success(), "attach failed: {out:?}");
        for replace in [false, true] {
            check_case_pair(
                &volume.mount.join(format!("replace-{replace}")),
                folding,
                replace,
            );
        }
    }
}

#[test]
fn link_arguments_are_opt_in_and_accept_only_links() {
    use transfer::LinkOptions;
    let cmd = || Cmd::new("export").args(transfer::export_args_with_links());
    let m = cmd()
        .try_get_matches_from(["export", "/", "out", "--preserve=links"])
        .unwrap();
    assert!(LinkOptions::from_matches(&m).unwrap().preserve_links);
    let m = cmd().try_get_matches_from(["export", "/", "out"]).unwrap();
    assert_eq!(
        LinkOptions::from_matches(&m).unwrap(),
        LinkOptions::default()
    );
    assert!(cmd()
        .try_get_matches_from(["export", "/", "out", "--preserve=all"])
        .is_err());
    let m = Cmd::new("export")
        .args(transfer::export_args_with_tree())
        .try_get_matches_from(["export", "/", "out"])
        .unwrap();
    assert_eq!(
        LinkOptions::from_matches(&m).unwrap(),
        LinkOptions::default()
    );
}

#[test]
fn a_revisited_directory_is_reported_as_loop_without_another_listing() {
    struct LoopTree(usize);
    impl TreeSource for LoopTree {
        fn metadata(&mut self, _: &Path) -> Result<TreeMetadata, CliError> {
            Ok(TreeMetadata {
                kind: TreeKind::Directory,
                size: 0,
                inode: 7,
                accessed: None,
                modified: None,
            })
        }
        fn read_dir(&mut self, _: &Path) -> Result<Vec<TreeEntry>, CliError> {
            self.0 += 1;
            assert_eq!(self.0, 1, "the repeated inode was enumerated");
            Ok(vec![TreeEntry {
                name: "again".into(),
                metadata: self.metadata(Path::new(""))?,
            }])
        }
        fn read_file_at(&mut self, _: &Path, _: u64, _: &mut [u8]) -> Result<usize, CliError> {
            panic!("a directory cannot be read as a file")
        }
    }
    let (root, _, dest) = scratch("links-loop");
    for preserve_links in [false, true] {
        let dest = dest.with_extension(preserve_links.to_string());
        let mut source = LoopTree(0);
        let out = transfer::export_tree_with_links(
            &request(&dest, false),
            &mut source,
            options(),
            transfer::LinkOptions { preserve_links },
        )
        .unwrap();
        assert_eq!(out.code, 1);
        let report = out.report.unwrap();
        assert_eq!(count(&report, "refused"), 1);
        assert!(report.to_compact().contains("loop"));
        assert_eq!(source.0, 1);
        assert!(!dest.join("again").exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::{
        ffi::OsStringExt,
        fs::{symlink, MetadataExt},
    };

    struct LinksHost {
        host: Host,
        reads: usize,
    }
    impl TreeSource for LinksHost {
        fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
            let mut m = self.host.metadata(p)?;
            m.inode = fs::symlink_metadata(self.host.root.join(p)).unwrap().ino();
            Ok(m)
        }
        fn read_dir(&mut self, p: &Path) -> Result<Vec<TreeEntry>, CliError> {
            let mut entries = self.host.read_dir(p)?;
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            for entry in &mut entries {
                entry.metadata = self.metadata(&p.join(&entry.name))?;
            }
            Ok(entries)
        }
        fn symbolic_link(&mut self, p: &Path) -> Result<Option<transfer::SymbolicLink>, CliError> {
            let p = self.host.root.join(p);
            if fs::symlink_metadata(&p).unwrap().file_type().is_symlink() {
                Ok(Some(transfer::SymbolicLink {
                    target: fs::read_link(p).unwrap(),
                    target_is_directory: false,
                }))
            } else {
                Ok(None)
            }
        }
        fn read_file_at(
            &mut self,
            p: &Path,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, CliError> {
            self.reads += 1;
            self.host.read_file_at(p, offset, buf)
        }
    }
    fn source(root: &Path) -> LinksHost {
        LinksHost {
            host: Host {
                root: root.into(),
                malicious: false,
            },
            reads: 0,
        }
    }
    fn export(host: &mut LinksHost, dest: &Path, replace: bool) -> Outcome {
        transfer::export_tree_with_links(
            &request(dest, replace),
            host,
            options(),
            transfer::LinkOptions {
                preserve_links: true,
            },
        )
        .unwrap()
    }
    fn hard_links(src: &Path) {
        fs::create_dir(src.join("sub")).unwrap();
        fs::write(src.join("a"), b"shared data").unwrap();
        fs::hard_link(src.join("a"), src.join("b")).unwrap();
        fs::hard_link(src.join("a"), src.join("sub/c")).unwrap();
    }

    #[test]
    fn three_names_share_one_destination_inode_and_one_source_read() {
        let (root, src, dest) = scratch("links-hard");
        hard_links(&src);
        let mut host = source(&src);
        let out = export(&mut host, &dest, false);
        assert_eq!(out.code, 0, "{:?}", out.report);
        assert_eq!(host.reads, 1);
        let first = fs::metadata(dest.join("a")).unwrap();
        assert_eq!(first.nlink(), 3);
        for name in ["a", "b", "sub/c"] {
            let m = fs::metadata(dest.join(name)).unwrap();
            assert_eq!((m.dev(), m.ino()), (first.dev(), first.ino()));
            assert_eq!(fs::read(dest.join(name)).unwrap(), b"shared data");
        }
        assert!(out
            .report
            .unwrap()
            .to_compact()
            .contains("hard link preserved"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_copies_hard_links_separately_and_refuses_symbolic_links() {
        let (root, src, dest) = scratch("links-default");
        hard_links(&src);
        symlink("a", src.join("symbolic")).unwrap();
        let mut host = source(&src);
        let out = transfer::export_tree(&request(&dest, false), &mut host, options()).unwrap();
        assert_eq!(out.code, 1);
        let report = out.report.unwrap();
        assert_eq!(count(&report, "refused"), 1);
        assert!(report
            .to_compact()
            .contains("hard link copied as separate file"));
        assert!(report.to_compact().contains("--preserve=links"));
        assert_eq!(host.reads, 3);
        for name in ["a", "b", "sub/c"] {
            assert_eq!(fs::metadata(dest.join(name)).unwrap().nlink(), 1);
        }
        assert!(fs::symlink_metadata(dest.join("symbolic")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symbolic_targets_remain_byte_identical_and_escape_warnings_do_not_follow_them() {
        let (root, src, dest) = scratch("links-symbolic");
        fs::create_dir(src.join("sub")).unwrap();
        let targets = [
            ("relative", PathBuf::from("missing")),
            ("absolute", root.join("outside")),
            ("escape", PathBuf::from("../outside")),
            ("sub/inside", PathBuf::from("../missing")),
            (
                "raw",
                PathBuf::from(OsString::from_vec(b"missing-\xff".to_vec())),
            ),
        ];
        for (name, target) in &targets {
            symlink(target, src.join(name)).unwrap();
        }
        let mut host = source(&src);
        let out = export(&mut host, &dest, false);
        assert_eq!(out.code, 0, "{:?}", out.report);
        assert_eq!(host.reads, 0);
        for (name, target) in &targets {
            assert_eq!(fs::read_link(dest.join(name)).unwrap(), *target);
        }
        let text = out.report.unwrap().to_compact();
        assert_eq!(text.matches("target escapes DEST").count(), 2, "{text}");
        assert!(!root.join("outside").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replace_publishes_both_link_types_over_existing_regular_files() {
        let (root, src, dest) = scratch("links-replace");
        hard_links(&src);
        symlink("missing", src.join("symbolic")).unwrap();
        fs::create_dir(&dest).unwrap();
        for name in ["a", "b", "symbolic"] {
            fs::write(dest.join(name), b"old").unwrap();
        }
        let out = export(&mut source(&src), &dest, true);
        assert_eq!(out.code, 0, "{:?}", out.report);
        assert_eq!(fs::metadata(dest.join("a")).unwrap().nlink(), 3);
        assert_eq!(
            fs::read_link(dest.join("symbolic")).unwrap(),
            Path::new("missing")
        );
        assert_eq!(
            fs::read_dir(&dest).unwrap().count(),
            4,
            "staging entries leaked"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn links_never_replace_unasked_or_redirect_through_destination_symlinks() {
        let (root, src, dest) = scratch("links-refusal");
        hard_links(&src);
        symlink("missing", src.join("symbolic")).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("b"), b"keep").unwrap();
        fs::create_dir(dest.join("symbolic")).unwrap();
        let out = export(&mut source(&src), &dest, false);
        assert_eq!(count(out.report.as_ref().unwrap(), "refused"), 2);
        assert_eq!(fs::read(dest.join("b")).unwrap(), b"keep");
        fs::remove_file(dest.join("b")).unwrap();
        symlink(root.join("outside"), dest.join("b")).unwrap();
        let out = export(&mut source(&src), &dest, true);
        assert_eq!(count(out.report.as_ref().unwrap(), "refused"), 2);
        assert!(!root.join("outside").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_first_copy_does_not_become_a_hard_link_anchor() {
        let (root, src, dest) = scratch("links-failed");
        fs::write(src.join("broken"), b"shared data").unwrap();
        fs::hard_link(src.join("broken"), src.join("good")).unwrap();
        fs::hard_link(src.join("broken"), src.join("last")).unwrap();
        let mut host = source(&src);
        let out = export(&mut host, &dest, false);
        assert_eq!(out.code, 1);
        assert_eq!(count(out.report.as_ref().unwrap(), "failed"), 1);
        assert_eq!(host.reads, 2);
        assert!(!dest.join("broken").exists());
        assert_eq!(fs::metadata(dest.join("good")).unwrap().nlink(), 2);
        assert_eq!(fs::read(dest.join("last")).unwrap(), b"shared data");
        fs::remove_dir_all(root).unwrap();
    }
}
