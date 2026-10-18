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
