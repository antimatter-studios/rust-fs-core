//! The kernel's hard links, symlinks and metadata are the identity oracle.
//! A refused transfer must not read the image or create even a staging file.

#![cfg(feature = "cli")]

use std::fs;
use std::path::{Path, PathBuf};

use fs_core::cli::transfer::{self, Destination, ExportOptions, FileSource};
use fs_core::cli::CliError;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("rust-fs-core-alias-{}-{tag}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("image"), b"unchanged image bytes").unwrap();
        Self(dir)
    }

    fn image(&self) -> PathBuf {
        self.0.join("image")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Source {
    image: PathBuf,
    offset: u64,
    reads: usize,
}

impl FileSource for Source {
    fn image_path(&self) -> Option<&Path> {
        Some(&self.image)
    }

    fn size(&self) -> u64 {
        3
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.reads += 1;
        let start = (self.offset + offset) as usize;
        let bytes = &b"abcXYZ"[start..];
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

fn names(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    names.sort();
    names
}

fn refused(dir: &Scratch, image: &Path, dest: &Path, offset: u64) {
    let original = fs::read(dir.image()).unwrap();
    let before = names(&dir.0);
    for replace in [false, true] {
        let mut source = Source {
            image: image.to_owned(),
            offset,
            reads: 0,
        };
        let err = transfer::export_file_with_options(
            &mut source,
            Destination::File {
                path: dest,
                replace,
            },
            ExportOptions { verify: true },
        )
        .unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.message.contains("same"), "{}", err.message);
        assert!(err.message.contains(&image.display().to_string()));
        assert!(err.message.contains(&dest.display().to_string()));
        assert_eq!(source.reads, 0);
        assert_eq!(fs::read(dir.image()).unwrap(), original);
        assert_eq!(names(&dir.0), before, "nothing, including staging, created");
    }
}

#[test]
fn exporting_to_the_image_itself_is_refused() {
    let dir = Scratch::new("same");
    refused(&dir, &dir.image(), &dir.image(), 0);
}

#[test]
fn exporting_to_a_hard_link_of_the_image_is_refused() {
    let dir = Scratch::new("hard-link");
    let dest = dir.0.join("hard-link");
    fs::hard_link(dir.image(), &dest).unwrap();
    refused(&dir, &dir.image(), &dest, 0);
}

#[cfg(unix)]
#[test]
fn exporting_to_a_symlink_chain_to_the_image_is_refused() {
    let dir = Scratch::new("symlink");
    let first = dir.0.join("first");
    let dest = dir.0.join("symlink");
    std::os::unix::fs::symlink("image", &first).unwrap();
    std::os::unix::fs::symlink("first", &dest).unwrap();
    refused(&dir, &dir.image(), &dest, 0);
}

#[cfg(unix)]
#[test]
fn a_symlinked_image_path_still_identifies_the_image() {
    let dir = Scratch::new("image-symlink");
    let image = dir.0.join("image-link");
    std::os::unix::fs::symlink("image", &image).unwrap();
    refused(&dir, &image, &dir.image(), 0);
}

#[test]
fn a_different_slice_offset_does_not_make_the_image_distinct() {
    let dir = Scratch::new("offset");
    refused(&dir, &dir.image(), &dir.image(), 3);
}

#[test]
fn every_local_path_in_a_tree_is_checked() {
    let dir = Scratch::new("tree");
    let tree = dir.0.join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("unrelated"), b"other file").unwrap();
    let alias = tree.join("image-alias");
    fs::hard_link(dir.image(), &alias).unwrap();
    transfer::refuse_image_alias(&dir.image(), &tree.join("unrelated")).unwrap();
    let err = transfer::refuse_image_alias(&dir.image(), &alias).unwrap_err();
    assert_eq!(err.code, 1);
    refused(&dir, &dir.image(), &alias, 0);
}

#[test]
fn the_same_guard_refuses_a_local_import_source() {
    let dir = Scratch::new("import-source");
    let local_source = dir.0.join("local-source");
    fs::hard_link(dir.image(), &local_source).unwrap();
    let err = transfer::refuse_image_alias(&dir.image(), &local_source).unwrap_err();
    assert_eq!(err.code, 1);
    assert!(err.message.contains(&local_source.display().to_string()));
}

#[test]
fn unrelated_existing_and_new_destinations_are_accepted() {
    let dir = Scratch::new("unrelated");
    let original = fs::read(dir.image()).unwrap();
    for replace in [false, true] {
        let dest = dir.0.join(format!("output-{replace}"));
        if replace {
            fs::write(&dest, b"previous").unwrap();
        }
        let mut source = Source {
            image: dir.image(),
            offset: 0,
            reads: 0,
        };
        transfer::export_file(
            &mut source,
            Destination::File {
                path: &dest,
                replace,
            },
        )
        .unwrap();
        assert_eq!(fs::read(dest).unwrap(), b"abc");
        assert_eq!(fs::read(dir.image()).unwrap(), original);
    }
}

#[test]
fn an_unreadable_identity_fails_before_any_copy_is_started() {
    let dir = Scratch::new("missing-image");
    let mut source = Source {
        image: dir.0.join("missing-image"),
        offset: 0,
        reads: 0,
    };
    let dest = dir.0.join("output");
    let before = names(&dir.0);
    let err = transfer::export_file(
        &mut source,
        Destination::File {
            path: &dest,
            replace: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, 1);
    assert_eq!(source.reads, 0);
    assert_eq!(names(&dir.0), before);
}
