//! Linux mountinfo identifies devices by major:minor, not mount-source names.
//! sysfs exposes loop backing files; compare inode identity, not path spelling.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use super::CliError;

#[derive(Default, Debug, PartialEq)]
pub(super) struct MountState {
    pub mounted: bool,
    pub read_write: bool,
}

pub(super) fn state(image: &Path) -> Result<MountState, CliError> {
    probe(image).map_err(|err| {
        CliError::failed(format!(
        "cannot determine host mount state for {} (Linux requires readable /proc and /sys): {err}",
        image.display()
    ))
    })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn probe(image: &Path) -> io::Result<MountState> {
    let image = fs::canonicalize(image)?;
    let metadata = fs::metadata(&image)?;
    let mut devices = HashSet::new();
    if metadata.file_type().is_block_device() {
        // Linux's dev_t layout (gnu_dev_major/gnu_dev_minor), including the
        // high bits used by large device numbers.
        let dev = metadata.rdev();
        let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
        let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
        collect(
            &PathBuf::from(format!("/sys/dev/block/{major}:{minor}")),
            &mut devices,
        )?;
    } else if !metadata.is_file() {
        return Err(invalid(
            "target is neither an image file nor a block device",
        ));
    }
    for entry in fs::read_dir("/sys/class/block")? {
        let device = entry?.path();
        let backing = device.join("loop/backing_file");
        if !backing.try_exists()? {
            continue;
        }
        let bytes = fs::read(backing)?;
        // sysfs appends one newline; preserve spaces and non-UTF-8 names.
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        if bytes.is_empty() {
            continue;
        }
        let backing = fs::metadata(Path::new(std::ffi::OsStr::from_bytes(bytes)))?;
        let same =
            if metadata.file_type().is_block_device() && backing.file_type().is_block_device() {
                metadata.rdev() == backing.rdev()
            } else {
                metadata.dev() == backing.dev() && metadata.ino() == backing.ino()
            };
        if same {
            collect(&device, &mut devices)?;
        }
    }
    mounts(&fs::read_to_string("/proc/self/mountinfo")?, &devices)
}

fn collect(device: &Path, devices: &mut HashSet<String>) -> io::Result<()> {
    let id = fs::read_to_string(device.join("dev"))?.trim().to_owned();
    if !devices.insert(id) {
        return Ok(());
    }
    // Whole-device imports must refuse even when only a partition is mounted.
    for entry in fs::read_dir(device)? {
        let entry = entry?;
        let child = entry.path();
        if entry.file_type()?.is_dir() && child.join("partition").try_exists()? {
            collect(&child, devices)?;
        }
    }
    // Device-mapper users (for example an encrypted loop) also make the
    // underlying bytes mutable. Follow holders without revisiting cycles.
    for entry in fs::read_dir(device.join("holders"))? {
        collect(&entry?.path(), devices)?;
    }
    Ok(())
}

fn mounts(mountinfo: &str, devices: &HashSet<String>) -> io::Result<MountState> {
    if mountinfo.is_empty() {
        return Err(invalid("empty mountinfo"));
    }
    let mut state = MountState::default();
    for line in mountinfo.lines() {
        let (mount, superblock) = line
            .split_once(" - ")
            .ok_or_else(|| invalid("malformed mountinfo separator"))?;
        let fields: Vec<_> = mount.split_whitespace().collect();
        let super_fields: Vec<_> = superblock.split_whitespace().collect();
        if fields.len() < 6 || super_fields.len() < 3 {
            return Err(invalid("malformed mountinfo fields"));
        }
        // These may use synthetic device IDs and several underlying disks.
        // Until their membership can be resolved, never infer "unmounted"
        // for a block target or associated image from a nonmatching ID.
        if !devices.is_empty() && matches!(super_fields[0], "btrfs" | "zfs") {
            return Err(invalid(
                "cannot resolve multi-device btrfs/zfs mount membership",
            ));
        }
        if devices.contains(fields[2]) {
            state.mounted = true;
            state.read_write |= fields[5].split(',').any(|o| o == "rw")
                && super_fields[2].split(',').any(|o| o == "rw");
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_matches_device_identity_and_both_read_only_flags() {
        let ids = HashSet::from(["7:0".to_owned()]);
        for (options, super_options, writable) in [
            ("rw,relatime", "rw", true),
            ("ro", "rw", false),
            ("rw", "ro", false),
        ] {
            // Field layout from proc_pid_mountinfo(5), including an optional
            // propagation field and an escaped mountpoint containing a space.
            let text = format!(
                "36 25 7:0 / /a\\040b {options} shared:1 - ext4 /dev/renamed {super_options}\n"
            );
            assert_eq!(
                mounts(&text, &ids).unwrap(),
                MountState {
                    mounted: true,
                    read_write: writable
                }
            );
            assert_eq!(
                mounts(&text, &HashSet::new()).unwrap(),
                MountState::default()
            );
        }
        assert!(mounts("broken", &ids).is_err());
        assert!(mounts("1 2 - ext4", &ids).is_err());
        assert!(mounts("", &ids).is_err());
        assert!(mounts("1 2 0:3 / / rw - btrfs /dev/loop0 rw", &ids).is_err());
    }
}
