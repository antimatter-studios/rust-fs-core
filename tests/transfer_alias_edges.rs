//! Identity edge cases use paths and device nodes created by the OS.
#![cfg(feature = "cli")]

use fs_core::cli::transfer::refuse_image_alias;
use std::fs;
use std::path::PathBuf;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rust-fs-core-alias-edge-{}-{tag}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("image"), b"image bytes").unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn identical_bytes_do_not_make_distinct_files_aliases() {
    let dir = Scratch::new("identical");
    fs::copy(dir.path("image"), dir.path("copy")).unwrap();
    refuse_image_alias(&dir.path("image"), &dir.path("copy")).unwrap();
}

#[test]
fn new_paths_are_allowed_but_missing_image_identity_is_an_error() {
    let dir = Scratch::new("missing");
    let new = dir.path("new");
    refuse_image_alias(&dir.path("image"), &new).unwrap();
    let err = refuse_image_alias(&dir.path("absent"), &new).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err
        .message
        .contains(&dir.path("absent").display().to_string()));
    assert!(err.message.contains(&new.display().to_string()));
    assert!(!new.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_parent_directories_resolve_to_the_same_image() {
    let dir = Scratch::new("parent");
    std::os::unix::fs::symlink(".", dir.path("parent")).unwrap();
    let alias = dir.path("parent/image");
    let err = refuse_image_alias(&dir.path("image"), &alias).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains("same"));
}

#[cfg(unix)]
#[test]
fn dangling_symlinks_are_allowed_but_resolution_errors_fail_closed() {
    let dir = Scratch::new("symlinks");
    std::os::unix::fs::symlink("missing", dir.path("dangling")).unwrap();
    refuse_image_alias(&dir.path("image"), &dir.path("dangling")).unwrap();
    std::os::unix::fs::symlink("loop", dir.path("loop")).unwrap();
    let err = refuse_image_alias(&dir.path("image"), &dir.path("loop")).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err
        .message
        .contains(&dir.path("image").display().to_string()));
    assert!(err
        .message
        .contains(&dir.path("loop").display().to_string()));
}

#[cfg(target_os = "linux")]
#[test]
fn distinct_device_nodes_compare_device_numbers_within_their_kind() {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::process::Command;

    let dir = Scratch::new("devices");
    // No device is opened or attached. mknod is the independent oracle for
    // distinct inodes identifying the same block/character device number.
    for (name, kind, minor) in [
        ("block", "b", "254"),
        ("block-alias", "b", "254"),
        ("different", "b", "255"),
        ("character", "c", "254"),
        ("character-alias", "c", "254"),
    ] {
        let out = Command::new("sudo")
            .args(["-n", "mknod"])
            .arg(dir.path(name))
            .args([kind, "7", minor])
            .output()
            .expect("device identity oracle needs sudo and coreutils mknod");
        assert!(out.status.success(), "sudo -n mknod: {out:?}");
    }
    let image = dir.path("block");
    let alias = dir.path("block-alias");
    let a = fs::metadata(&image).unwrap();
    let b = fs::metadata(&alias).unwrap();
    assert!(a.file_type().is_block_device());
    assert_ne!(a.ino(), b.ino());
    assert_eq!(a.rdev(), b.rdev());
    assert_eq!(refuse_image_alias(&image, &alias).unwrap_err().code, 1);
    refuse_image_alias(&image, &dir.path("different")).unwrap();
    refuse_image_alias(&image, &dir.path("character")).unwrap();
    assert_eq!(
        refuse_image_alias(&dir.path("character"), &dir.path("character-alias"))
            .unwrap_err()
            .code,
        1
    );
}
