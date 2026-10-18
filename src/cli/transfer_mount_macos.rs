//! macOS mounts name BSD disk nodes, not image files. `hdiutil info` maps
//! attached images to their nodes; compare inode identity, not path spelling.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::process::Command;

use super::CliError;

#[derive(Default, Debug, PartialEq)]
pub(super) struct MountState {
    pub mounted: bool,
    pub read_write: bool,
}

pub(super) fn state(image: &Path) -> Result<MountState, CliError> {
    probe(image).map_err(|err| {
        CliError::failed(format!(
            "cannot determine host mount state for {} (macOS requires hdiutil, diskutil and mount): {err}",
            image.display()
        ))
    })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn tool(program: &str, args: &[&str]) -> io::Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(invalid(format!("{program} failed: {}", output.status)));
    }
    String::from_utf8(output.stdout).map_err(|_| invalid(format!("{program} output is not UTF-8")))
}

fn probe(image: &Path) -> io::Result<MountState> {
    let image = fs::canonicalize(image)?;
    let metadata = fs::metadata(&image)?;
    let kind = metadata.file_type();
    let mut disks = HashSet::new();
    // Raw character nodes (rdiskN) and block nodes (diskN) share one disk.
    let target = if kind.is_block_device() || kind.is_char_device() {
        let name = image
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let name = name.strip_prefix('r').unwrap_or(name);
        if !name.starts_with("disk") {
            return Err(invalid("target is neither an image file nor a disk device"));
        }
        disks.insert(name.to_owned());
        Some(name.to_owned())
    } else if metadata.is_file() {
        None
    } else {
        return Err(invalid("target is neither an image file nor a disk device"));
    };
    for attached in attached(&tool("/usr/bin/hdiutil", &["info"])?) {
        let same = match &target {
            Some(name) => attached.nodes.iter().any(|node| node == name),
            None => {
                let path = attached
                    .image
                    .as_deref()
                    .ok_or_else(|| invalid("hdiutil info lists an image without image-path"))?;
                backs(path, &metadata)?
            }
        };
        // An attached image's nodes include any APFS container synthesized
        // from it, so every node of a matching image is a candidate mount.
        if same {
            disks.extend(attached.nodes);
        }
    }
    if !disks.is_empty() {
        synthesized(&tool("/usr/sbin/diskutil", &["list"])?, &mut disks);
    }
    mounts(&tool("/sbin/mount", &[])?, &disks)
}

// hdiutil records the path an image was attached from. A recorded path that
// no longer exists names no file, so it cannot name the target, which does;
// like a path that now names another file, such an image is not recognised.
fn backs(path: &str, target: &fs::Metadata) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(backing) => Ok(backing.dev() == target.dev() && backing.ino() == target.ino()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

struct Attached {
    image: Option<String>,
    nodes: Vec<String>,
}

fn attached(info: &str) -> Vec<Attached> {
    let mut images: Vec<Attached> = Vec::new();
    for line in info.lines() {
        if line.starts_with("====") {
            images.push(Attached {
                image: None,
                nodes: Vec::new(),
            });
            continue;
        }
        let Some(current) = images.last_mut() else {
            continue;
        };
        if let Some(field) = line.strip_prefix("image-path") {
            current.image = field.split_once(": ").map(|(_, path)| path.to_owned());
        } else if let Some(node) = line
            .split_whitespace()
            .next()
            .and_then(|node| node.strip_prefix("/dev/"))
        {
            current.nodes.push(node.to_owned());
        }
    }
    images
}

// A whole-disk target must refuse when only a slice is mounted.
fn slice_of(name: &str, disk: &str) -> bool {
    name == disk
        || name
            .strip_prefix(disk)
            .and_then(|rest| rest.strip_prefix('s'))
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

// APFS volumes mount from a synthesized disk whose physical store is the
// target's slice. Follow those stores, like Linux holders, until stable.
fn synthesized(list: &str, disks: &mut HashSet<String>) {
    loop {
        let mut current = None;
        let mut added = false;
        for line in list.lines() {
            if let Some(header) = line.strip_prefix("/dev/") {
                current = header.split_whitespace().next().map(str::to_owned);
            } else if let (Some(disk), Some((_, stores))) =
                (&current, line.split_once("Physical Store "))
            {
                let backed = stores
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|store| !store.is_empty())
                    .any(|store| disks.iter().any(|known| slice_of(store, known)));
                if backed && disks.insert(disk.trim_end_matches(':').to_owned()) {
                    added = true;
                }
            }
        }
        if !added {
            return;
        }
    }
}

fn mounts(table: &str, disks: &HashSet<String>) -> io::Result<MountState> {
    if table.is_empty() {
        return Err(invalid("empty mount table"));
    }
    let mut state = MountState::default();
    for line in table.lines() {
        // mount(8): "<source> on <mount point> (<type>, <option>, ...)".
        let (source, rest) = line
            .split_once(" on ")
            .ok_or_else(|| invalid("malformed mount table line"))?;
        let options = rest
            .rsplit_once(" (")
            .and_then(|(_, options)| options.strip_suffix(')'))
            .ok_or_else(|| invalid("malformed mount table options"))?;
        let Some(name) = source.strip_prefix("/dev/") else {
            continue;
        };
        if disks.iter().any(|disk| slice_of(name, disk)) {
            state.mounted = true;
            state.read_write |= !options.split(", ").any(|option| option == "read-only");
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_table_matches_slices_and_the_read_only_option() {
        let disks = HashSet::from(["disk4".to_owned()]);
        for (options, writable) in [
            ("hfs, local, nodev", true),
            ("hfs, local, read-only", false),
        ] {
            let text = format!("/dev/disk4s1 on /Volumes/a on b ({options})\n");
            assert_eq!(
                mounts(&text, &disks).unwrap(),
                MountState {
                    mounted: true,
                    read_write: writable
                }
            );
            assert_eq!(
                mounts(&text, &HashSet::from(["disk41".to_owned()])).unwrap(),
                MountState::default()
            );
        }
        assert!(mounts("broken", &disks).is_err());
        assert!(mounts("/dev/disk4 on /x", &disks).is_err());
        assert!(mounts("", &disks).is_err());
    }

    #[test]
    fn an_attached_image_whose_recorded_path_is_gone_is_not_the_target() {
        let dir = std::env::temp_dir().join(format!("rust-fs-core-backs-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let target = dir.join("target");
        let other = dir.join("other");
        fs::write(&target, b"target").unwrap();
        fs::write(&other, b"other").unwrap();
        let metadata = fs::metadata(&target).unwrap();
        let gone = dir.join("gone");
        assert!(backs(target.to_str().unwrap(), &metadata).unwrap());
        assert!(!backs(other.to_str().unwrap(), &metadata).unwrap());
        assert!(!backs(gone.to_str().unwrap(), &metadata).unwrap());
        let below_a_file = other.join("child");
        assert!(backs(below_a_file.to_str().unwrap(), &metadata).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hdiutil_info_and_apfs_stores_resolve_disk_nodes() {
        let info = "framework : 1\n================================================\nimage-path      : /a b/volume.dmg\nimage-type      : read/write\n/dev/disk4\tGUID_partition_scheme\t\n/dev/disk4s1\tApple_APFS\t\n";
        let images = attached(info);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].image.as_deref(), Some("/a b/volume.dmg"));
        assert_eq!(images[0].nodes, ["disk4", "disk4s1"]);
        let list = "/dev/disk5 (synthesized):\n   0:      APFS Container Scheme -   +16 MB   disk5\n                                 Physical Store disk4s1\n";
        let mut disks = HashSet::from(["disk4".to_owned()]);
        synthesized(list, &mut disks);
        assert!(disks.contains("disk5"));
    }
}
