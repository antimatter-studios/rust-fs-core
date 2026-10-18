//! Durable records use Python's JSON reader as an independent serialization oracle.
#![cfg(feature = "cli")]

use fs_core::cli::transfer::manifest::{
    Identity, LocalIdentity, Manifest, ManifestEntry, Timestamp,
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
