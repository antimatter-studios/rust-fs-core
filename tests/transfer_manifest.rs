//! Durable records use Python's JSON reader as an independent serialization oracle.
#![cfg(feature = "cli")]

use fs_core::cli::transfer::manifest::{
    Identity, LocalIdentity, Manifest, ManifestEntry, ManifestRead, ResumeDecision, Timestamp,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
#[path = "support/fork_holder.rs"]
mod fork_holder;

#[cfg(unix)]
#[test]
fn dropped_manifests_release_locks_while_a_forked_child_is_alive() {
    let file = scratch("fork-holder");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&entry(), true).unwrap();
    let child = fork_holder::ForkHolder::new();
    drop(manifest);
    let reopened = Manifest::open(&file);
    drop(child);
    let (_, read) = reopened.unwrap();
    assert_eq!(read.entries, [entry()]);
}

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("core-manifest-{}-{tag}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root.join("manifest.jsonl")
}

fn export_paths(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let manifest = scratch(tag);
    let root = manifest.parent().unwrap().to_path_buf();
    let image = root.join("image");
    fs::write(&image, b"image must remain intact").unwrap();
    (manifest, image, root.join("destination"))
}

#[test]
fn export_manifest_opens_outside_existing_and_missing_destination_trees() {
    for existing in [false, true] {
        let (manifest, image, dest) = export_paths(if existing {
            "safe-existing"
        } else {
            "safe-new"
        });
        if existing {
            fs::create_dir(&dest).unwrap();
            fs::write(dest.join("unrelated"), b"keep").unwrap();
        }
        let (ledger, read) = Manifest::open_for_export(&manifest, &image, &dest, true).unwrap();
        assert!(read.entries.is_empty());
        assert!(!read.torn_tail);
        drop(ledger);
        assert_eq!(dest.exists(), existing);
        assert!(fs::read(&manifest).unwrap().is_empty());
    }
}

#[test]
fn export_manifest_refuses_image_names_and_hard_links_without_recovery() {
    let (manifest, image, dest) = export_paths("image-alias");
    fs::hard_link(&image, &manifest).unwrap();
    for alias in [&image, &manifest] {
        assert!(Manifest::open_for_export(alias, &image, &dest, true).is_err());
        assert_eq!(fs::read(alias).unwrap(), b"image must remain intact");
    }
    assert!(!dest.exists());
}

#[test]
fn export_manifest_refuses_single_destination_names_and_hard_links() {
    let (manifest, image, dest) = export_paths("single-alias");
    assert!(Manifest::open_for_export(&dest, &image, &dest, false).is_err());
    assert!(!dest.exists());
    fs::write(&dest, b"published contents").unwrap();
    fs::hard_link(&dest, &manifest).unwrap();
    assert!(Manifest::open_for_export(&manifest, &image, &dest, false).is_err());
    assert_eq!(fs::read(&dest).unwrap(), b"published contents");
}

#[test]
fn export_manifest_refuses_containment_even_before_destination_creation() {
    let (_, image, dest) = export_paths("containment");
    for manifest in [
        dest.clone(),
        dest.join("ledger"),
        dest.join("new/../ledger"),
    ] {
        assert!(Manifest::open_for_export(&manifest, &image, &dest, true).is_err());
        assert!(!dest.exists());
    }
    fs::create_dir(&dest).unwrap();
    assert!(Manifest::open_for_export(&dest.join("ledger"), &image, &dest, true).is_err());
    assert!(!dest.join("ledger").exists());
}

#[test]
fn export_manifest_refuses_hard_links_to_nested_destination_files() {
    let (manifest, image, dest) = export_paths("tree-hard-link");
    fs::create_dir_all(dest.join("nested")).unwrap();
    let published = dest.join("nested/file");
    fs::write(&published, b"earlier export").unwrap();
    fs::hard_link(&published, &manifest).unwrap();
    assert!(Manifest::open_for_export(&manifest, &image, &dest, true).is_err());
    assert_eq!(fs::read(&published).unwrap(), b"earlier export");
}

#[test]
fn export_manifest_identity_errors_and_nonregular_paths_fail_before_creation() {
    let (manifest, image, dest) = export_paths("identity-error");
    assert!(
        Manifest::open_for_export(&manifest, &image.with_extension("missing"), &dest, true)
            .is_err()
    );
    assert!(!manifest.exists());
    fs::create_dir(&manifest).unwrap();
    assert!(Manifest::open_for_export(&manifest, &image, &dest, true).is_err());
    assert!(!dest.exists());
}

#[cfg(unix)]
#[test]
fn export_manifest_refuses_symlink_image_and_tree_file_aliases() {
    use std::os::unix::fs::symlink;
    for tree in [false, true] {
        let (manifest, image, dest) = export_paths(if tree {
            "tree-symlink"
        } else {
            "image-symlink"
        });
        let target = if tree {
            fs::create_dir(&dest).unwrap();
            let target = dest.join("file");
            fs::write(&target, b"earlier export").unwrap();
            target
        } else {
            image.clone()
        };
        symlink(&target, &manifest).unwrap();
        let before = fs::read(&target).unwrap();
        assert!(Manifest::open_for_export(&manifest, &image, &dest, true).is_err());
        assert_eq!(fs::read(&target).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn export_manifest_resolves_symlinked_parents_before_containment_checks() {
    use std::os::unix::fs::symlink;
    let (manifest, image, dest) = export_paths("parent-symlink");
    let root = manifest.parent().unwrap();
    fs::create_dir(&dest).unwrap();
    let alias = root.join("alias");
    symlink(&dest, &alias).unwrap();
    assert!(Manifest::open_for_export(&alias.join("ledger"), &image, &dest, true).is_err());
    assert!(Manifest::open_for_export(&dest.join("ledger"), &image, &alias, true).is_err());
    assert!(!dest.join("ledger").exists());
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    let dangling_dest = alias.join("new-tree");
    assert!(
        Manifest::open_for_export(&dest.join("new-tree/ledger"), &image, &dangling_dest, true)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn export_manifest_refuses_destination_symlinks_to_an_external_ledger() {
    use std::os::unix::fs::symlink;
    let (manifest, image, dest) = export_paths("external-ledger");
    fs::create_dir(&dest).unwrap();
    fs::write(&manifest, b"not a record; must not recover").unwrap();
    symlink(&manifest, dest.join("file")).unwrap();
    assert!(Manifest::open_for_export(&manifest, &image, &dest, true).is_err());
    assert_eq!(
        fs::read(&manifest).unwrap(),
        b"not a record; must not recover"
    );
}
fn import_paths(tag: &str, tree: bool) -> (PathBuf, PathBuf, PathBuf) {
    let (manifest, image, source) = export_paths(tag);
    if tree {
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"source must remain intact").unwrap();
    } else {
        fs::write(&source, b"source must remain intact").unwrap();
    }
    (manifest, image, source)
}

#[test]
fn import_manifest_opens_external_ledgers_and_reports_recovery() {
    for tree in [false, true] {
        let (manifest, image, source) = import_paths(
            if tree {
                "import-safe-tree"
            } else {
                "import-safe-file"
            },
            tree,
        );
        let (ledger, read) = Manifest::open_for_import(&manifest, &image, &source, tree).unwrap();
        assert!(read.entries.is_empty());
        assert!(!read.torn_tail);
        drop(ledger);
        fs::write(&manifest, b"torn record").unwrap();
        let (ledger, read) = Manifest::open_for_import(&manifest, &image, &source, tree).unwrap();
        assert!(read.torn_tail);
        drop(ledger);
        assert!(fs::read(&manifest).unwrap().is_empty());
        let file = if tree { source.join("file") } else { source };
        assert_eq!(fs::read(file).unwrap(), b"source must remain intact");
        assert_eq!(fs::read(image).unwrap(), b"image must remain intact");
    }
}

#[test]
fn import_manifest_refuses_image_names_and_hard_links_without_recovery() {
    let (manifest, image, source) = import_paths("import-image-alias", false);
    fs::hard_link(&image, &manifest).unwrap();
    for alias in [&image, &manifest] {
        assert!(Manifest::open_for_import(alias, &image, &source, false).is_err());
        assert_eq!(fs::read(alias).unwrap(), b"image must remain intact");
    }
    assert_eq!(fs::read(source).unwrap(), b"source must remain intact");
}

#[test]
fn import_manifest_refuses_single_source_names_and_hard_links() {
    let (manifest, image, source) = import_paths("import-source-alias", false);
    fs::hard_link(&source, &manifest).unwrap();
    for alias in [&source, &manifest] {
        assert!(Manifest::open_for_import(alias, &image, &source, false).is_err());
        assert_eq!(fs::read(alias).unwrap(), b"source must remain intact");
    }
    assert_eq!(fs::read(image).unwrap(), b"image must remain intact");
}

#[test]
fn import_manifest_refuses_source_tree_containment_before_creation() {
    let (_, image, source) = import_paths("import-containment", true);
    for manifest in [
        source.clone(),
        source.join("ledger"),
        source.join("new/../ledger"),
    ] {
        assert!(Manifest::open_for_import(&manifest, &image, &source, true).is_err());
    }
    assert!(!source.join("ledger").exists());
    assert!(!source.join("new").exists());
    assert_eq!(
        fs::read(source.join("file")).unwrap(),
        b"source must remain intact"
    );
}

#[test]
fn import_manifest_refuses_hard_links_to_nested_source_files() {
    let (manifest, image, source) = import_paths("import-tree-hard-link", true);
    fs::create_dir(source.join("nested")).unwrap();
    let file = source.join("nested/another");
    fs::write(&file, b"nested source must remain intact").unwrap();
    fs::hard_link(&file, &manifest).unwrap();
    assert!(Manifest::open_for_import(&manifest, &image, &source, true).is_err());
    assert_eq!(fs::read(file).unwrap(), b"nested source must remain intact");
}

#[test]
fn import_manifest_identity_errors_and_nonregular_ledgers_fail_before_creation() {
    let (manifest, image, source) = import_paths("import-identity-error", false);
    assert!(
        Manifest::open_for_import(&manifest, &image.with_extension("missing"), &source, false)
            .is_err()
    );
    assert!(!manifest.exists());
    assert!(
        Manifest::open_for_import(&manifest, &image, &source.with_extension("missing"), false)
            .is_err()
    );
    assert!(!manifest.exists());
    fs::create_dir(&manifest).unwrap();
    assert!(Manifest::open_for_import(&manifest, &image, &source, false).is_err());
    assert_eq!(fs::read(source).unwrap(), b"source must remain intact");
}

#[cfg(unix)]
#[test]
fn import_manifest_refuses_symlinks_to_image_and_source_files() {
    use std::os::unix::fs::symlink;
    for target_kind in 0..3 {
        let (manifest, image, source) =
            import_paths(&format!("import-symlink-{target_kind}"), target_kind == 2);
        let target = match target_kind {
            0 => image.clone(),
            1 => source.clone(),
            _ => source.join("file"),
        };
        symlink(&target, &manifest).unwrap();
        let before = fs::read(&target).unwrap();
        assert!(Manifest::open_for_import(&manifest, &image, &source, target_kind == 2).is_err());
        assert_eq!(fs::read(target).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn import_manifest_resolves_symlinked_source_ancestors_before_containment() {
    use std::os::unix::fs::symlink;
    let (manifest, image, source) = import_paths("import-parent-symlink", true);
    let alias = manifest.with_file_name("alias");
    symlink(&source, &alias).unwrap();
    assert!(Manifest::open_for_import(&alias.join("ledger"), &image, &source, true).is_err());
    assert!(Manifest::open_for_import(&source.join("ledger"), &image, &alias, true).is_err());
    assert!(!source.join("ledger").exists());
}

#[cfg(unix)]
#[test]
fn import_manifest_refuses_source_symlinks_to_external_ledgers_and_dangling_paths() {
    use std::os::unix::fs::symlink;
    let (manifest, image, source) = import_paths("import-external-ledger", true);
    fs::write(&manifest, b"torn record must remain intact").unwrap();
    symlink(&manifest, source.join("alias")).unwrap();
    assert!(Manifest::open_for_import(&manifest, &image, &source, true).is_err());
    assert_eq!(
        fs::read(&manifest).unwrap(),
        b"torn record must remain intact"
    );
    let dangling = manifest.with_file_name("dangling");
    symlink(source.join("missing"), &dangling).unwrap();
    assert!(Manifest::open_for_import(&dangling, &image, &source, true).is_err());
    assert!(!source.join("missing").exists());
}

fn entry() -> ManifestEntry {
    let time = Timestamp {
        seconds: 123,
        nanoseconds: 456,
    };
    let local = LocalIdentity {
        path: PathBuf::from("local\n\"file"),
        device: 7,
        inode: 8,
        size: 3,
        accessed: time,
        modified: time,
        changed: time,
    };
    ManifestEntry {
        source: Identity::Image {
            path: PathBuf::from("/image/file"),
            image: local.clone(),
            offset: 4096,
            filesystem_uuid: "abcd-1234".into(),
            inode: 99,
        },
        destination: Identity::Local(local),
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        completed_at: time,
    }
}
#[test]
fn only_verified_completions_are_appended_and_reopened_in_order() {
    let file = scratch("verified");
    let (mut manifest, read) = Manifest::open(&file).unwrap();
    assert!(read.entries.is_empty());
    assert!(!read.torn_tail);
    assert!(!manifest.append(&entry(), false).unwrap());
    assert!(fs::read(&file).unwrap().is_empty());
    assert!(manifest.append(&entry(), true).unwrap());
    let mut second = entry();
    second.bytes = 0;
    second.sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into();
    assert!(manifest.append(&second, true).unwrap());
    drop(manifest);
    let (_, read) = Manifest::open(&file).unwrap();
    assert_eq!(read.entries, [entry(), second]);
    assert!(!read.torn_tail);
}
#[test]
fn identity_and_digest_fields_are_read_by_an_independent_json_parser() {
    let file = scratch("json");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&entry(), true).unwrap();
    drop(manifest);
    let script = r#"import json,sys
with open(sys.argv[1], encoding='utf-8') as f:
    lines=f.readlines()
assert len(lines)==1 and lines[0].endswith('\n')
e=json.loads(lines[0])
assert e['source']['kind']=='image'
assert e['source']['path']=='/image/file' and e['source']['offset']==4096
assert e['source']['filesystem_uuid']=='abcd-1234' and e['source']['inode']==99
assert e['destination']['kind']=='local'
l={k:v for k,v in e['destination'].items() if k!='kind'}
assert e['source']['image']==l
assert l['path']=='local\n"file' and (l['device'],l['inode'],l['size'])==(7,8,3)
assert all(l[k]=={'seconds':123,'nanoseconds':456} for k in ['accessed','modified','changed'])
assert e['completed_at']=={'seconds':123,'nanoseconds':456}
assert e['bytes']==3 and e['sha256']=='ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
"#;
    let result = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-c", script])
        .arg(&file)
        .output()
        .expect("install Python 3 for the independent JSON manifest oracle");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn a_torn_tail_is_reported_and_removed_before_the_next_append() {
    let file = scratch("torn");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&entry(), true).unwrap();
    drop(manifest);
    let complete = fs::read(&file).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{\"source\":")
        .unwrap();
    let (mut manifest, read) = Manifest::open(&file).unwrap();
    assert!(read.torn_tail);
    assert_eq!(read.entries, [entry()]);
    assert_eq!(fs::read(&file).unwrap(), complete);
    manifest.append(&entry(), true).unwrap();
    drop(manifest);
    let (_, read) = Manifest::open(&file).unwrap();
    assert!(!read.torn_tail);
    assert_eq!(read.entries.len(), 2);
}
#[test]
fn a_complete_json_record_without_its_newline_is_still_a_torn_tail() {
    let file = scratch("no-newline");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&entry(), true).unwrap();
    drop(manifest);
    let mut bytes = fs::read(&file).unwrap();
    bytes.pop();
    fs::write(&file, bytes).unwrap();
    let (_, read) = Manifest::open(&file).unwrap();
    assert!(read.torn_tail);
    assert!(read.entries.is_empty());
    assert!(fs::read(&file).unwrap().is_empty());
}
#[test]
fn malformed_complete_records_fail_without_truncating_anything() {
    let file = scratch("invalid");
    for bytes in [b"not json\n".as_slice(), b"{}\n", b"\n"] {
        fs::write(&file, bytes).unwrap();
        assert!(Manifest::open(&file).is_err());
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }
    let mut invalid = entry();
    invalid.sha256 = "x".repeat(64);
    let bytes = format!("{}\n", serde_json::to_string(&invalid).unwrap());
    fs::write(&file, &bytes).unwrap();
    assert!(Manifest::open(&file).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), bytes);
    let oversized = vec![b'x'; 64 * 1024 + 1];
    fs::write(&file, &oversized).unwrap();
    assert!(Manifest::open(&file).is_err());
    assert_eq!(fs::read(&file).unwrap(), oversized);
}
#[test]
fn concurrent_manifest_writers_are_refused_without_touching_the_file() {
    let file = scratch("locked");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&entry(), true).unwrap();
    assert!(Manifest::open(&file).is_err());
    drop(manifest);
    let (_, read) = Manifest::open(&file).unwrap();
    assert_eq!(read.entries, [entry()]);
}
#[test]
fn invalid_digest_and_timestamp_entries_are_refused_before_writing() {
    let file = scratch("invalid-entry");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    let mut invalid = entry();
    invalid.sha256 = "not a digest".into();
    assert!(manifest.append(&invalid, true).is_err());
    invalid = entry();
    invalid.completed_at.nanoseconds = 1_000_000_000;
    assert!(manifest.append(&invalid, true).is_err());
    invalid = entry();
    let Identity::Local(local) = &mut invalid.destination else {
        unreachable!()
    };
    local.accessed.nanoseconds = 1_000_000_000;
    assert!(manifest.append(&invalid, true).is_err());
    invalid = entry();
    let Identity::Image { path, .. } = &mut invalid.source else {
        unreachable!()
    };
    *path = PathBuf::from("x".repeat(64 * 1024));
    assert!(manifest.append(&invalid, true).is_err());
    assert!(fs::read(&file).unwrap().is_empty());
}
#[test]
fn nonexistent_parent_is_an_error_without_creating_directories() {
    let file = scratch("missing-parent")
        .with_file_name("missing")
        .join("ledger");
    assert!(Manifest::open(&file).is_err());
    assert!(!file.parent().unwrap().exists());
}

fn evidence(entries: Vec<ManifestEntry>) -> ManifestRead {
    ManifestRead {
        entries,
        torn_tail: false,
    }
}

// Driver identities are supplied independently of the local publication.
struct ExportSource {
    image: PathBuf,
    ledger: PathBuf,
    reads: usize,
    verified: bool,
}
impl fs_core::cli::transfer::FileSource for ExportSource {
    fn image_path(&self) -> Option<&std::path::Path> {
        Some(&self.image)
    }
    fn manifest_identity(&self) -> Result<Identity, fs_core::cli::CliError> {
        assert!(
            self.verified,
            "unverified copies need no identity capability"
        );
        let mut identity = entry().source;
        if let Identity::Image { image, .. } = &mut identity {
            image.path = self.image.clone();
        }
        Ok(identity)
    }
    fn size(&self) -> u64 {
        3
    }
    fn read_at(&mut self, _: u64, buf: &mut [u8]) -> Result<usize, fs_core::cli::CliError> {
        self.reads += 1;
        buf[..3].copy_from_slice(b"abc");
        Ok(3)
    }
}
impl fs_core::cli::transfer::TreeSource for ExportSource {
    fn image_path(&self) -> Option<&std::path::Path> {
        Some(&self.image)
    }
    fn manifest_identity(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Identity, fs_core::cli::CliError> {
        use fs_core::cli::transfer::FileSource;
        let mut identity = FileSource::manifest_identity(self)?;
        if let Identity::Image { path: recorded, .. } = &mut identity {
            *recorded = path.into();
        }
        Ok(identity)
    }
    fn metadata(
        &mut self,
        path: &std::path::Path,
    ) -> Result<fs_core::cli::transfer::TreeMetadata, fs_core::cli::CliError> {
        use fs_core::cli::transfer::{TreeKind, TreeMetadata};
        Ok(TreeMetadata {
            kind: if path == std::path::Path::new("/image/file") {
                TreeKind::Directory
            } else {
                TreeKind::File
            },
            size: 3,
            inode: if path.ends_with("later") { 2 } else { 1 },
            accessed: None,
            modified: None,
        })
    }
    fn read_dir(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<fs_core::cli::transfer::TreeEntry>, fs_core::cli::CliError> {
        Ok(["first", "later"]
            .into_iter()
            .map(|name| fs_core::cli::transfer::TreeEntry {
                name: name.into(),
                metadata: self.metadata(&path.join(name)).unwrap(),
            })
            .collect())
    }
    fn read_file_at(
        &mut self,
        path: &std::path::Path,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, fs_core::cli::CliError> {
        use fs_core::cli::transfer::FileSource;
        if path.ends_with("later") {
            let lines = fs::read_to_string(&self.ledger).unwrap();
            assert_eq!(
                lines.lines().count(),
                2 * usize::from(self.verified),
                "first publication must be persisted before next read"
            );
            assert!(Manifest::open(&self.ledger).is_err(), "ledger stays locked");
            return Err(fs_core::cli::CliError::failed("later read failed"));
        }
        FileSource::read_at(self, offset, buf)
    }
}
fn export_run(tag: &str, verify: bool, tree: bool) -> (PathBuf, PathBuf, fs_core::cli::Outcome) {
    use fs_core::cli::transfer::{
        self, ExportManifestOptions, ExportOptions, ExportRequest, Output, TreeOptions,
    };
    let (ledger, image, dest) = export_paths(tag);
    if tree && verify {
        let (mut previous, _) = Manifest::open(&ledger).unwrap();
        previous.append(&entry(), true).unwrap();
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)
        .unwrap()
        .write_all(b"torn tail")
        .unwrap();
    let mut source = ExportSource {
        image,
        ledger: ledger.clone(),
        reads: 0,
        verified: verify,
    };
    let request = ExportRequest {
        path: "/image/file".into(),
        output: Output::File(dest.clone()),
        replace: false,
    };
    let manifest = ExportManifestOptions {
        manifest: Some(ledger.clone()),
        resume: false,
    };
    let outcome = if tree {
        transfer::export_tree_with_manifest(
            &request,
            &mut source,
            TreeOptions {
                recursive: true,
                verify,
                ..TreeOptions::default()
            },
            &manifest,
        )
    } else {
        transfer::export_with_manifest(&request, &mut source, ExportOptions { verify }, &manifest)
    }
    .unwrap();
    assert_eq!(
        outcome.report.as_ref().unwrap().get("manifest_torn_tail"),
        Some(&fs_core::cli::Json::Bool(true))
    );
    (ledger, dest, outcome)
}

#[test]
fn verified_file_exports_capture_published_host_and_driver_identities() {
    let (ledger, dest, outcome) = export_run("record-file", true, false);
    assert_eq!(outcome.code, 0);
    let (_, read) = Manifest::open(&ledger).unwrap();
    assert_eq!(read.entries.len(), 1);
    let recorded = &read.entries[0];
    let mut expected = entry().source;
    if let Identity::Image { image, .. } = &mut expected {
        image.path = ledger.with_file_name("image");
    }
    assert_eq!(recorded.source, expected);
    assert_eq!(recorded.bytes, 3);
    assert_eq!(recorded.sha256, entry().sha256);
    let Identity::Local(local) = &recorded.destination else {
        panic!("local destination")
    };
    assert_eq!(local.path, dest);
    assert_eq!(local.size, fs::metadata(&dest).unwrap().len());
    let script = r#"import json,os,sys
e=json.loads(open(sys.argv[1]).read()); s=os.stat(sys.argv[2]); d=e['destination']
assert (d['device'],d['inode'],d['size'])==(s.st_dev,s.st_ino,s.st_size)
changed=s.st_ctime_ns
if os.name=='nt':
    import ctypes,msvcrt
    from ctypes import wintypes
    get=ctypes.WinDLL('kernel32',use_last_error=True).GetFileInformationByHandleEx
    get.argtypes=[wintypes.HANDLE,ctypes.c_int,ctypes.c_void_p,wintypes.DWORD]
    basic=ctypes.create_string_buffer(40)
    with open(sys.argv[2],'rb') as f:
        assert get(msvcrt.get_osfhandle(f.fileno()),0,basic,len(basic))
    changed=(ctypes.c_longlong.from_buffer(basic,24).value-116444736000000000)*100
for field,ns in [('accessed',s.st_atime_ns),('modified',s.st_mtime_ns),('changed',changed)]:
    assert d[field]['seconds']*10**9+d[field]['nanoseconds']==ns
"#;
    let result = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-c", script])
        .arg(ledger)
        .arg(dest)
        .output()
        .expect("install Python 3 for host stat oracle");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn unverified_file_and_tree_exports_append_no_evidence() {
    for tree in [false, true] {
        let (ledger, dest, _) = export_run(
            if tree {
                "unverified-tree"
            } else {
                "unverified-file"
            },
            false,
            tree,
        );
        assert!(fs::read(ledger).unwrap().is_empty());
        assert_eq!(
            fs::read(if tree { dest.join("first") } else { dest }).unwrap(),
            b"abc"
        );
    }
}

#[test]
fn tree_exports_persist_each_verified_file_before_a_later_failure() {
    let (ledger, dest, outcome) = export_run("record-tree", true, true);
    assert_ne!(outcome.code, 0);
    let (_, read) = Manifest::open(&ledger).unwrap();
    assert_eq!(read.entries.len(), 2);
    assert_eq!(read.entries[0], entry());
    let Identity::Image { path, .. } = &read.entries[1].source else {
        panic!("image source")
    };
    assert_eq!(path, &PathBuf::from("/image/file/first"));
    assert_eq!(fs::read(dest.join("first")).unwrap(), b"abc");
    assert!(!dest.join("later").exists());
}

#[test]
fn manifest_adapters_refuse_unsafe_paths_before_any_transfer_reads() {
    use fs_core::cli::transfer::{
        self, ExportManifestOptions, ExportOptions, ExportRequest, Output, TreeOptions,
    };
    for tree in [false, true] {
        let (ledger, image, dest) = export_paths(if tree {
            "adapter-unsafe-tree"
        } else {
            "adapter-unsafe-file"
        });
        for unsafe_path in [
            image.clone(),
            dest.clone(),
            if tree {
                dest.join("ledger")
            } else {
                dest.clone()
            },
        ] {
            let mut source = ExportSource {
                image: image.clone(),
                ledger: ledger.clone(),
                reads: 0,
                verified: true,
            };
            let request = ExportRequest {
                path: "/image/file".into(),
                output: Output::File(dest.clone()),
                replace: true,
            };
            let manifest = ExportManifestOptions {
                manifest: Some(unsafe_path),
                resume: false,
            };
            let result = if tree {
                transfer::export_tree_with_manifest(
                    &request,
                    &mut source,
                    TreeOptions {
                        recursive: true,
                        verify: true,
                        ..TreeOptions::default()
                    },
                    &manifest,
                )
            } else {
                transfer::export_with_manifest(
                    &request,
                    &mut source,
                    ExportOptions { verify: true },
                    &manifest,
                )
            };
            assert!(result.is_err());
            assert_eq!(source.reads, 0);
            assert!(!dest.exists());
        }
    }
}

#[test]
fn export_resume_arguments_preserve_tree_and_metadata_options() {
    use fs_core::cli::transfer::{self, ExportManifestOptions, ExportRequest, TreeOptions};
    let matches = fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args_with_resume())
        .try_get_matches_from([
            "export",
            "/tree",
            "destination",
            "--resume",
            "--manifest",
            "ledger.jsonl",
            "--recursive",
            "--max-depth",
            "12",
            "--verify",
            "--replace",
            "--preserve=times,links,sparse,xattrs",
        ])
        .unwrap();
    let options = ExportManifestOptions::from_matches(&matches).unwrap();
    assert_eq!(options.manifest, Some(PathBuf::from("ledger.jsonl")));
    assert!(options.resume);
    assert!(ExportRequest::from_matches(&matches).unwrap().replace);
    let tree = TreeOptions::from_matches(&matches).unwrap();
    assert!(tree.recursive && tree.verify);
    assert_eq!(tree.max_depth, 12);
    assert!(
        transfer::LinkOptions::from_matches(&matches)
            .unwrap()
            .preserve_links
    );
    assert!(
        transfer::TimeOptions::from_matches(&matches)
            .unwrap()
            .preserve_times
    );
    assert!(
        transfer::SparseOptions::from_matches(&matches)
            .unwrap()
            .preserve_sparse
    );
    assert!(
        transfer::XattrOptions::from_matches(&matches)
            .unwrap()
            .preserve_xattrs
    );
}

#[test]
fn export_resume_arguments_require_a_manifest_even_with_custom_parsers() {
    use fs_core::cli::clap::{Arg, ArgAction};
    use fs_core::cli::transfer::{self, ExportManifestOptions};
    let error = fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args_with_resume())
        .try_get_matches_from(["export", "/file", "destination", "--resume"])
        .unwrap_err();
    assert_eq!(
        error.kind(),
        fs_core::cli::clap::error::ErrorKind::MissingRequiredArgument
    );
    let matches = fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args())
        .arg(Arg::new("resume").long("resume").action(ArgAction::SetTrue))
        .try_get_matches_from(["export", "/file", "destination", "--resume"])
        .unwrap();
    let error = ExportManifestOptions::from_matches(&matches).unwrap_err();
    assert_eq!(error.code, 2);
    assert!(error.message.contains("--manifest"));
}

#[test]
fn export_manifest_and_resume_arguments_refuse_stdout() {
    use fs_core::cli::transfer::{self, ExportManifestOptions, ExportRequest};
    for resume in [false, true] {
        let mut args = vec!["export", "/file", "-", "--manifest", "ledger.jsonl"];
        if resume {
            args.push("--resume");
        }
        let matches = fs_core::cli::clap::Command::new("export")
            .args(transfer::export_args_with_resume())
            .try_get_matches_from(args)
            .unwrap();
        let error = ExportManifestOptions::from_matches(&matches).unwrap_err();
        assert_eq!(error.code, 2);
        assert!(error.message.contains("stdout"));
        assert!(ExportRequest::from_matches(&matches).is_err());
    }
}

#[test]
fn export_manifest_arguments_accept_single_files_without_resume_or_verify() {
    use fs_core::cli::transfer::{self, ExportManifestOptions, ExportOptions};
    let matches = fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args_with_manifest())
        .try_get_matches_from([
            "export",
            "/file",
            "destination",
            "--manifest",
            "ledger.jsonl",
        ])
        .unwrap();
    let options = ExportManifestOptions::from_matches(&matches).unwrap();
    assert_eq!(options.manifest, Some(PathBuf::from("ledger.jsonl")));
    assert!(!options.resume);
    assert!(!ExportOptions::from_matches(&matches).unwrap().verify);
    assert!(fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args_with_resume())
        .try_get_matches_from(["export", "/file", "destination", "--manifest"])
        .is_err());
}

#[test]
fn export_manifest_options_default_for_all_legacy_argument_sets() {
    use fs_core::cli::transfer::{self, ExportManifestOptions};
    let commands = [
        fs_core::cli::clap::Command::new("export").args(transfer::export_args()),
        fs_core::cli::clap::Command::new("export").args(transfer::export_args_with_verify()),
        fs_core::cli::clap::Command::new("export").args(transfer::export_args_with_tree()),
        fs_core::cli::clap::Command::new("export").args(transfer::export_args_with_xattrs()),
        fs_core::cli::clap::Command::new("export").args(transfer::export_args_with_resume()),
    ];
    for command in commands {
        let matches = command
            .try_get_matches_from(["export", "/file", "-"])
            .unwrap();
        assert_eq!(
            ExportManifestOptions::from_matches(&matches).unwrap(),
            ExportManifestOptions::default()
        );
    }
}

#[cfg(unix)]
#[test]
fn export_manifest_arguments_keep_non_unicode_paths() {
    use fs_core::cli::transfer::{self, ExportManifestOptions};
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let ledger = OsString::from_vec(b"ledger-\xff.jsonl".to_vec());
    let matches = fs_core::cli::clap::Command::new("export")
        .args(transfer::export_args_with_resume())
        .try_get_matches_from([
            OsString::from("export"),
            OsString::from("/file"),
            OsString::from("destination"),
            OsString::from("--resume"),
            OsString::from("--manifest"),
            ledger.clone(),
        ])
        .unwrap();
    assert_eq!(
        ExportManifestOptions::from_matches(&matches)
            .unwrap()
            .manifest,
        Some(PathBuf::from(ledger))
    );
}

#[test]
fn resume_redigests_unchanged_bytes_and_returns_the_earlier_time() {
    let recorded = entry();
    let mut source = std::io::Cursor::new(b"abc");
    let file = scratch("resume-reopened");
    let (mut manifest, _) = Manifest::open(&file).unwrap();
    manifest.append(&recorded, true).unwrap();
    drop(manifest);
    let (_, read) = Manifest::open(&file).unwrap();
    assert_eq!(
        read.check_resume(&recorded.source, &recorded.destination, &mut source)
            .unwrap(),
        ResumeDecision::Skipped {
            completed_at: recorded.completed_at
        }
    );
    assert_eq!(source.position(), 3, "the source must actually be re-read");
}

#[test]
fn resume_recopies_a_same_size_same_mtime_rewrite() {
    let file = scratch("resume-rewrite").with_file_name("source");
    fs::write(&file, b"abc").unwrap();
    let before = fs::metadata(&file).unwrap();
    fs::write(&file, b"xyz").unwrap();
    OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = fs::metadata(&file).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    let recorded = entry();
    // Supply identical snapshots deliberately: only the fresh digest can catch it.
    assert_eq!(
        evidence(vec![recorded.clone()])
            .check_resume(
                &recorded.source,
                &recorded.destination,
                &mut fs::File::open(file).unwrap()
            )
            .unwrap(),
        ResumeDecision::Copy
    );
}

struct MustNotRead;
impl std::io::Read for MustNotRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("ineligible evidence must not read the source")
    }
}

#[test]
fn resume_without_a_record_copies_without_reading() {
    let recorded = entry();
    assert_eq!(
        evidence(vec![])
            .check_resume(&recorded.source, &recorded.destination, &mut MustNotRead)
            .unwrap(),
        ResumeDecision::Copy
    );
}

#[test]
fn resume_recopies_each_changed_local_destination_identity_field() {
    let recorded = entry();
    for field in 0..6 {
        let mut destination = recorded.destination.clone();
        let Identity::Local(local) = &mut destination else {
            unreachable!()
        };
        match field {
            0 => local.path.push("different"),
            1 => local.device += 1,
            2 => local.inode += 1,
            3 => local.size += 1,
            4 => local.modified.seconds += 1,
            _ => local.changed.seconds += 1,
        }
        assert_eq!(
            evidence(vec![recorded.clone()])
                .check_resume(&recorded.source, &destination, &mut MustNotRead)
                .unwrap(),
            ResumeDecision::Copy,
            "changed destination field {field}"
        );
    }
}

#[test]
fn resume_ignores_access_time_changes_but_still_reads_the_source() {
    let recorded = entry();
    let mut source = recorded.source.clone();
    let Identity::Image { image, .. } = &mut source else {
        unreachable!()
    };
    image.accessed.seconds += 1;
    let mut destination = recorded.destination.clone();
    let Identity::Local(local) = &mut destination else {
        unreachable!()
    };
    local.accessed.seconds += 1;
    assert_eq!(
        evidence(vec![recorded.clone()])
            .check_resume(&source, &destination, &mut &b"abc"[..])
            .unwrap(),
        ResumeDecision::Skipped {
            completed_at: recorded.completed_at
        }
    );
}

#[test]
fn resume_recopies_changed_image_identity_and_source_identity() {
    let mut recorded = entry();
    std::mem::swap(&mut recorded.source, &mut recorded.destination);
    for field in 0..6 {
        let mut destination = recorded.destination.clone();
        let Identity::Image {
            path,
            image,
            offset,
            filesystem_uuid,
            inode,
        } = &mut destination
        else {
            unreachable!()
        };
        match field {
            0 => path.push("different"),
            1 => image.inode += 1,
            2 => *offset += 1,
            3 => filesystem_uuid.push('x'),
            4 => *inode += 1,
            _ => image.changed.seconds += 1,
        }
        assert_eq!(
            evidence(vec![recorded.clone()])
                .check_resume(&recorded.source, &destination, &mut MustNotRead)
                .unwrap(),
            ResumeDecision::Copy
        );
    }
    let mut source = recorded.source.clone();
    let Identity::Local(local) = &mut source else {
        unreachable!()
    };
    local.inode += 1;
    assert_eq!(
        evidence(vec![recorded.clone()])
            .check_resume(&source, &recorded.destination, &mut MustNotRead)
            .unwrap(),
        ResumeDecision::Copy
    );
}

#[test]
fn resume_never_falls_back_to_superseded_destination_evidence() {
    let recorded = entry();
    let mut latest = recorded.clone();
    latest.sha256 = "0".repeat(64);
    assert_eq!(
        evidence(vec![recorded.clone(), latest.clone()])
            .check_resume(&recorded.source, &recorded.destination, &mut &b"abc"[..])
            .unwrap(),
        ResumeDecision::Copy
    );
    latest.sha256 = recorded.sha256.clone();
    latest.completed_at.seconds += 1;
    assert_eq!(
        evidence(vec![recorded.clone(), latest.clone()])
            .check_resume(&recorded.source, &recorded.destination, &mut &b"abc"[..])
            .unwrap(),
        ResumeDecision::Skipped {
            completed_at: latest.completed_at
        }
    );
    latest.source = recorded.destination.clone();
    assert_eq!(
        evidence(vec![recorded.clone(), latest])
            .check_resume(&recorded.source, &recorded.destination, &mut MustNotRead)
            .unwrap(),
        ResumeDecision::Copy
    );
}

#[test]
fn resume_restarts_short_or_extra_sources_and_propagates_read_errors() {
    let recorded = entry();
    let read = evidence(vec![recorded.clone()]);
    for bytes in [b"ab".as_slice(), b"abcd"] {
        assert_eq!(
            read.check_resume(&recorded.source, &recorded.destination, &mut &bytes[..])
                .unwrap(),
            ResumeDecision::Copy
        );
    }
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("resume read failed"))
        }
    }
    let error = read
        .check_resume(&recorded.source, &recorded.destination, &mut Broken)
        .unwrap_err();
    assert!(error.message.contains("resume read failed"));
}

#[test]
fn resume_streams_bounded_chunks_against_python_hashlib() {
    let file = scratch("resume-oracle").with_file_name("source");
    let bytes = vec![0x71; 2 * fs_core::cli::transfer::CHUNK + 7];
    fs::write(&file, &bytes).unwrap();
    let output = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args([
            "-c",
            "import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())",
        ])
        .arg(file)
        .output()
        .expect("install Python 3 for the independent resume digest oracle");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut recorded = entry();
    recorded.bytes = bytes.len() as u64;
    let Identity::Local(local) = &mut recorded.destination else {
        unreachable!()
    };
    local.size = recorded.bytes;
    recorded.sha256 = String::from_utf8(output.stdout).unwrap().trim().into();
    struct Bounded<'a>(&'a [u8]);
    impl std::io::Read for Bounded<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            assert!(buffer.len() <= fs_core::cli::transfer::CHUNK);
            // Force short reads as well as multiple chunks.
            let n = buffer.len().min(17_003);
            std::io::Read::read(&mut self.0, &mut buffer[..n])
        }
    }
    assert_eq!(
        evidence(vec![recorded.clone()])
            .check_resume(
                &recorded.source,
                &recorded.destination,
                &mut Bounded(&bytes)
            )
            .unwrap(),
        ResumeDecision::Skipped {
            completed_at: recorded.completed_at
        }
    );
}

#[test]
fn resume_retries_interrupted_reads_and_handles_empty_files() {
    let mut recorded = entry();
    recorded.bytes = 0;
    recorded.sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into();
    let Identity::Local(local) = &mut recorded.destination else {
        unreachable!()
    };
    local.size = 0;
    struct InterruptedOnce(bool);
    impl std::io::Read for InterruptedOnce {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::replace(&mut self.0, false) {
                Err(std::io::ErrorKind::Interrupted.into())
            } else {
                Ok(0)
            }
        }
    }
    assert_eq!(
        evidence(vec![recorded.clone()])
            .check_resume(
                &recorded.source,
                &recorded.destination,
                &mut InterruptedOnce(true)
            )
            .unwrap(),
        ResumeDecision::Skipped {
            completed_at: recorded.completed_at
        }
    );
}

// Resume endpoints: host metadata of the real image and destination files,
// with a driver inode the test can change while the source is digested.
struct ResumeSource {
    image: PathBuf,
    dest: PathBuf,
    content: Vec<u8>,
    inode: u64,
    offsets: Vec<u64>,
    during_digest: Option<fn(&mut ResumeSource)>,
}
impl fs_core::cli::transfer::FileSource for ResumeSource {
    fn image_path(&self) -> Option<&std::path::Path> {
        Some(&self.image)
    }
    fn manifest_identity(&self) -> Result<Identity, fs_core::cli::CliError> {
        let file = fs::File::open(&self.image).unwrap();
        Ok(Identity::Image {
            path: PathBuf::from("/image/file"),
            image: LocalIdentity::capture(&self.image, &file)?,
            offset: 0,
            filesystem_uuid: "resume-uuid".into(),
            inode: self.inode,
        })
    }
    fn size(&self) -> u64 {
        self.content.len() as u64
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, fs_core::cli::CliError> {
        self.offsets.push(offset);
        if let Some(mutate) = self.during_digest.take() {
            mutate(self);
        }
        let start = offset as usize;
        let n = buf.len().min(self.content.len() - start);
        buf[..n].copy_from_slice(&self.content[start..start + n]);
        Ok(n)
    }
}

fn resume_export(
    source: &mut ResumeSource,
    ledger: &std::path::Path,
    resume: bool,
) -> fs_core::cli::Outcome {
    use fs_core::cli::transfer::{
        self, ExportManifestOptions, ExportOptions, ExportRequest, Output,
    };
    let request = ExportRequest {
        path: "/image/file".into(),
        output: Output::File(source.dest.clone()),
        replace: true,
    };
    let manifest = ExportManifestOptions {
        manifest: Some(ledger.into()),
        resume,
    };
    transfer::export_with_manifest(&request, source, ExportOptions { verify: true }, &manifest)
        .unwrap()
}

// A verified, recorded first export, so the second run has evidence to weigh.
fn resumable(tag: &str, record: bool) -> (PathBuf, ResumeSource) {
    let (ledger, image, dest) = export_paths(tag);
    fs::File::create(&ledger).unwrap();
    let mut source = ResumeSource {
        image,
        dest,
        content: b"resume bytes".to_vec(),
        inode: 5,
        offsets: Vec::new(),
        during_digest: None,
    };
    if record {
        resume_export(&mut source, &ledger, false);
        assert_eq!(records(&ledger), 1);
    }
    source.offsets.clear();
    (ledger, source)
}

fn records(ledger: &std::path::Path) -> usize {
    Manifest::open(ledger).unwrap().1.entries.len()
}

// Python's hashlib digests the published file independently of sha2.
fn host_sha256(path: &std::path::Path) -> String {
    let result = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args([
            "-c",
            "import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())",
        ])
        .arg(path)
        .output()
        .expect("install Python 3 for the SHA-256 oracle");
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap().trim().into()
}

// A restart copies from byte zero, verifies, publishes and records again.
fn assert_recopied(
    outcome: &fs_core::cli::Outcome,
    ledger: &std::path::Path,
    source: &ResumeSource,
    entries: usize,
) {
    use fs_core::cli::Json;
    let report = outcome.report.as_ref().unwrap();
    assert_eq!(report.get("skipped"), None);
    assert_eq!(report.get("verified"), Some(&Json::Bool(true)));
    assert_eq!(fs::read(&source.dest).unwrap(), source.content);
    assert_eq!(records(ledger), entries);
    let (_, read) = Manifest::open(ledger).unwrap();
    assert_eq!(
        read.entries.last().unwrap().sha256,
        host_sha256(&source.dest)
    );
}

#[test]
fn resume_skips_an_unchanged_single_file_with_the_exact_earlier_time() {
    use fs_core::cli::Json;
    let (ledger, mut source) = resumable("resume-skip", true);
    let before = fs::read(&ledger).unwrap();
    let modified = fs::metadata(&source.dest).unwrap().modified().unwrap();
    OpenOptions::new()
        .append(true)
        .open(&ledger)
        .unwrap()
        .write_all(b"torn tail")
        .unwrap();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_eq!(outcome.code, 0);
    assert!(!source.offsets.is_empty(), "the source is digested again");
    let report = outcome.report.as_ref().unwrap();
    assert_eq!(report.get("skipped"), Some(&Json::from("verified earlier")));
    assert_eq!(report.get("verified"), Some(&Json::Bool(false)));
    assert_eq!(report.get("manifest_torn_tail"), Some(&Json::Bool(true)));
    assert_eq!(fs::read(&ledger).unwrap(), before, "a skip appends nothing");
    assert_eq!(
        fs::metadata(&source.dest).unwrap().modified().unwrap(),
        modified
    );
    // The earlier completion time comes from the ledger through Python's JSON reader.
    let script = "import json,sys;t=json.loads(open(sys.argv[1]).read().splitlines()[-1])['completed_at'];print(t['seconds'],t['nanoseconds'])";
    let result = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-c", script])
        .arg(&ledger)
        .output()
        .expect("install Python 3 for the JSON oracle");
    let stdout = String::from_utf8(result.stdout).unwrap();
    let (seconds, nanoseconds) = stdout.trim().split_once(' ').unwrap();
    let (seconds, nanoseconds): (i64, i64) =
        (seconds.parse().unwrap(), nanoseconds.parse().unwrap());
    let at = report.get("verified_earlier_at").unwrap();
    assert_eq!(at.get("seconds"), Some(&Json::Int(seconds)));
    assert_eq!(at.get("nanoseconds"), Some(&Json::Int(nanoseconds)));
    let text = outcome.text.as_deref().unwrap();
    assert!(
        text.contains(&format!(
            "skipped: verified earlier at {seconds}.{nanoseconds:09}"
        )),
        "{text}"
    );
    assert!(text.contains("not verified now"), "{text}");
}

#[test]
fn resume_recopies_a_same_size_same_mtime_source_rewrite() {
    let (ledger, mut source) = resumable("resume-rewrite", true);
    source.content = b"rewritten!!!".to_vec();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_recopied(&outcome, &ledger, &source, 2);
}

#[test]
fn resume_copies_without_a_record_or_with_superseded_evidence() {
    let (ledger, mut source) = resumable("resume-missing", false);
    fs::write(&source.dest, b"stale").unwrap();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_recopied(&outcome, &ledger, &source, 1);

    // A later record for the same destination from another source wins.
    let (ledger, mut source) = resumable("resume-superseded", true);
    let (mut manifest, read) = Manifest::open(&ledger).unwrap();
    let mut later = read.entries[0].clone();
    if let Identity::Image { inode, .. } = &mut later.source {
        *inode += 1;
    }
    manifest.append(&later, true).unwrap();
    drop(manifest);
    let outcome = resume_export(&mut source, &ledger, true);
    assert_recopied(&outcome, &ledger, &source, 3);
}

#[test]
fn resume_recopies_changed_and_missing_destinations() {
    let (ledger, mut source) = resumable("resume-dest-changed", true);
    OpenOptions::new()
        .append(true)
        .open(&source.dest)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_recopied(&outcome, &ledger, &source, 2);

    let (ledger, mut source) = resumable("resume-dest-missing", true);
    fs::remove_file(&source.dest).unwrap();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_recopied(&outcome, &ledger, &source, 2);
}

#[test]
fn resume_restarts_partial_destinations_at_byte_zero() {
    let (ledger, mut source) = resumable("resume-partial", true);
    OpenOptions::new()
        .write(true)
        .open(&source.dest)
        .unwrap()
        .set_len(4)
        .unwrap();
    let outcome = resume_export(&mut source, &ledger, true);
    assert_eq!(source.offsets.first(), Some(&0));
    assert_recopied(&outcome, &ledger, &source, 2);
}

#[test]
fn resume_restarts_when_an_endpoint_changes_during_the_digest() {
    let mutations: [fn(&mut ResumeSource); 2] = [
        |source| source.inode += 1,
        |source| {
            OpenOptions::new()
                .append(true)
                .open(&source.dest)
                .unwrap()
                .write_all(b"!")
                .unwrap()
        },
    ];
    for (n, mutate) in mutations.into_iter().enumerate() {
        let (ledger, mut source) = resumable(&format!("resume-mutated-{n}"), true);
        source.during_digest = Some(mutate);
        let outcome = resume_export(&mut source, &ledger, true);
        assert!(source.during_digest.is_none(), "the digest read the source");
        assert_recopied(&outcome, &ledger, &source, 2);
    }
}
