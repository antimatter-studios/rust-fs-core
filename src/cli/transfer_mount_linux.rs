//! Linux mountinfo identifies devices by major:minor, not mount-source names.
//! sysfs exposes loop backing files; compare inode identity, not path spelling.
//! Btrfs reports a synthetic ID per subvolume; /sys/fs/btrfs lists members.
//! ZFS names datasets by pool; `zpool status -P` lists each pool's vdevs.
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
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
        collect(
            &PathBuf::from(format!("/sys/dev/block/{}", dev_id(metadata.rdev()))),
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
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    let members = if devices.is_empty() || !mountinfo.contains(" - btrfs ") {
        HashSet::new()
    } else {
        btrfs_members(Path::new("/sys/fs/btrfs"), &devices)?
    };
    // An imported pool writes its vdevs with no dataset mounted, so ask
    // whenever the module is loaded, not only when a dataset is visible.
    let pools = if mountinfo.contains(" - zfs ") || Path::new("/sys/module/zfs").try_exists()? {
        zfs_pools(&zpool_status()?, &|vdev| {
            let vdev = fs::metadata(vdev)?;
            if vdev.file_type().is_block_device() {
                Ok(devices.contains(&dev_id(vdev.rdev())))
            } else if vdev.is_file() {
                Ok(metadata.is_file()
                    && metadata.dev() == vdev.dev()
                    && metadata.ino() == vdev.ino())
            } else {
                Err(invalid("zfs vdev is neither a block device nor a file"))
            }
        })?
    } else {
        HashSet::new()
    };
    mounts(&mountinfo, &devices, &members, &pools, &|source| {
        let metadata = fs::metadata(source)?;
        if !metadata.file_type().is_block_device() {
            return Err(invalid(format!(
                "btrfs source {source} is not a block device"
            )));
        }
        Ok(dev_id(metadata.rdev()))
    })
}

fn dev_id(dev: u64) -> String {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
    format!("{major}:{minor}")
}

/// Every device of each mounted Btrfs filesystem that has a target device
/// among its members; the kernel removes /sys/fs/btrfs/<fsid> at unmount.
fn btrfs_members(sysfs: &Path, devices: &HashSet<String>) -> io::Result<HashSet<String>> {
    let mut members = HashSet::new();
    for entry in fs::read_dir(sysfs)? {
        let list = entry?.path().join("devices");
        // Entries such as `features` describe the module, not a filesystem.
        if !list.try_exists()? {
            continue;
        }
        let mut ids = HashSet::new();
        for member in fs::read_dir(list)? {
            ids.insert(
                fs::read_to_string(member?.path().join("dev"))?
                    .trim()
                    .to_owned(),
            );
        }
        if !ids.is_disjoint(devices) {
            members.extend(ids);
        }
    }
    Ok(members)
}

fn zpool_status() -> io::Result<String> {
    let output = Command::new("zpool")
        .args(["status", "-P"])
        .output()
        .map_err(|err| invalid(format!("cannot resolve zfs pool membership: zpool: {err}")))?;
    if !output.status.success() {
        return Err(invalid(format!(
            "cannot resolve zfs pool membership: zpool status -P: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| invalid("zpool status -P is not UTF-8"))
}

/// Every imported pool with a vdev for which `ours` holds; each `pool:`
/// header starts a pool, and its config rows name vdevs by absolute path.
fn zfs_pools(
    status: &str,
    ours: &dyn Fn(&Path) -> io::Result<bool>,
) -> io::Result<HashSet<String>> {
    let mut pools = HashSet::new();
    let mut pool = None;
    for line in status.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("pool:") {
            pool = Some(name.trim().to_owned());
        } else if let Some(vdev) = line
            .split_whitespace()
            .next()
            .filter(|w| w.starts_with('/'))
        {
            let pool = pool
                .as_ref()
                .ok_or_else(|| invalid("zfs vdev outside any pool"))?;
            if ours(Path::new(vdev))? {
                pools.insert(pool.clone());
            }
        }
    }
    Ok(pools)
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

fn mounts(
    mountinfo: &str,
    devices: &HashSet<String>,
    members: &HashSet<String>,
    pools: &HashSet<String>,
    source_id: &dyn Fn(&str) -> io::Result<String>,
) -> io::Result<MountState> {
    if mountinfo.is_empty() {
        return Err(invalid("empty mountinfo"));
    }
    let mut state = MountState::default();
    let mut btrfs_seen = false;
    let mut zfs_seen = false;
    for line in mountinfo.lines() {
        let (mount, superblock) = line
            .split_once(" - ")
            .ok_or_else(|| invalid("malformed mountinfo separator"))?;
        let fields: Vec<_> = mount.split_whitespace().collect();
        let super_fields: Vec<_> = superblock.split_whitespace().collect();
        if fields.len() < 6 || super_fields.len() < 3 {
            return Err(invalid("malformed mountinfo fields"));
        }
        // A Btrfs mount's ID is synthetic; its source names one member.
        // A ZFS source is a dataset or snapshot whose name starts with its pool.
        let matched = match super_fields[0] {
            "btrfs" => !members.is_empty() && members.contains(&source_id(super_fields[1])?),
            "zfs" => super_fields[1]
                .split(['/', '@'])
                .next()
                .is_some_and(|pool| pools.contains(pool)),
            _ => devices.contains(fields[2]),
        };
        btrfs_seen |= matched && super_fields[0] == "btrfs";
        zfs_seen |= matched && super_fields[0] == "zfs";
        if matched {
            state.mounted = true;
            state.read_write |= fields[5].split(',').any(|o| o == "rw")
                && super_fields[2].split(',').any(|o| o == "rw");
        }
    }
    // A member of a filesystem mounted outside this mount namespace: its
    // writability is unknown here, so assume the worst.
    // The same holds for an imported pool with no dataset visible here.
    if (!members.is_empty() && !btrfs_seen) || (!pools.is_empty() && !zfs_seen) {
        state.mounted = true;
        state.read_write = true;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unresolved(source: &str) -> io::Result<String> {
        Err(invalid(format!("unexpected source lookup {source}")))
    }

    #[test]
    fn mountinfo_matches_device_identity_and_both_read_only_flags() {
        let ids = HashSet::from(["7:0".to_owned()]);
        let none = HashSet::new();
        let mounts =
            |text: &str, ids: &HashSet<String>| mounts(text, ids, &none, &none, &unresolved);
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
        // Neither Btrfs nor ZFS refuses every block target any longer: a
        // filesystem with no target among its members is unrelated.
        assert_eq!(
            mounts("1 2 0:3 / / rw - btrfs /dev/loop0 rw", &ids).unwrap(),
            MountState::default()
        );
        assert_eq!(
            mounts("1 2 0:3 / / rw - zfs pool/ds rw", &ids).unwrap(),
            MountState::default()
        );
    }

    #[test]
    fn btrfs_mounts_match_by_member_source_not_synthetic_id() {
        let ids = HashSet::from(["7:1".to_owned()]);
        let members = HashSet::from(["7:0".to_owned(), "7:1".to_owned()]);
        let none = HashSet::new();
        let source = |source: &str| match source {
            "/dev/loop0" => Ok("7:0".to_owned()),
            "/dev/sda1" => Ok("8:1".to_owned()),
            _ => Err(invalid("gone")),
        };
        for (options, writable) in [("rw", true), ("ro", false)] {
            let text = format!("1 2 0:40 / /m {options} - btrfs /dev/loop0 rw\n");
            assert_eq!(
                mounts(&text, &ids, &members, &none, &source).unwrap(),
                MountState {
                    mounted: true,
                    read_write: writable
                }
            );
        }
        // Another Btrfs filesystem's line, while ours is mounted elsewhere.
        let other = "1 2 0:41 / /o ro - btrfs /dev/sda1 ro\n";
        assert_eq!(
            mounts(other, &ids, &members, &none, &source).unwrap(),
            MountState {
                mounted: true,
                read_write: true
            }
        );
        assert!(mounts(
            "1 2 0:42 / /g rw - btrfs /dev/gone rw\n",
            &ids,
            &members,
            &none,
            &source
        )
        .is_err());
    }

    #[test]
    fn zfs_mounts_match_by_pool_named_in_zpool_status() {
        // Layout from zpool-status(8) with -P, including a mirror row, a
        // second pool and the trailing errors line.
        let status = "  pool: tank\n state: ONLINE\nconfig:\n\n\tNAME            STATE     READ WRITE CKSUM\n\ttank            ONLINE       0     0     0\n\t  mirror-0      ONLINE       0     0     0\n\t    /dev/loop0p1  ONLINE     0     0     0\n\t    /dev/loop1p1  ONLINE     0     0     0\n\nerrors: No known data errors\n\n  pool: other\n state: ONLINE\nconfig:\n\n\tNAME          STATE     READ WRITE CKSUM\n\tother         ONLINE       0     0     0\n\t  /dev/sda3   ONLINE       0     0     0\n\nerrors: No known data errors\n";
        let ours = |vdev: &Path| match vdev.to_str() {
            Some("/dev/loop1p1") => Ok(true),
            Some("/dev/loop0p1" | "/dev/sda3") => Ok(false),
            _ => Err(invalid("gone")),
        };
        let pools = zfs_pools(status, &ours).unwrap();
        assert_eq!(pools, HashSet::from(["tank".to_owned()]));
        assert!(zfs_pools("\t/dev/gone ONLINE\n", &|_| Ok(true)).is_err());
        assert!(zfs_pools("  pool: tank\n\t/dev/gone ONLINE\n", &ours).is_err());
        assert!(zfs_pools("no pools available\n", &ours).unwrap().is_empty());

        let ids = HashSet::from(["7:1".to_owned()]);
        let none = HashSet::new();
        for (text, writable) in [
            ("1 2 0:50 / /t rw - zfs tank rw,xattr\n", true),
            ("1 2 0:51 / /t/ds ro - zfs tank/ds ro,xattr\n", false),
            ("1 2 0:52 / /s ro - zfs tank/ds@snap ro\n", false),
        ] {
            assert_eq!(
                mounts(text, &ids, &none, &pools, &unresolved).unwrap(),
                MountState {
                    mounted: true,
                    read_write: writable
                }
            );
        }
        // A dataset of another pool, or a pool whose name only shares a prefix.
        for text in [
            "1 2 0:53 / /o ro - zfs other/ds ro\n",
            "1 2 0:54 / /o ro - zfs tanker ro\n",
        ] {
            assert_eq!(
                mounts(text, &ids, &none, &pools, &unresolved).unwrap(),
                MountState {
                    mounted: true,
                    read_write: true
                }
            );
            assert_eq!(
                mounts(text, &ids, &none, &none, &unresolved).unwrap(),
                MountState::default()
            );
        }
    }
}
