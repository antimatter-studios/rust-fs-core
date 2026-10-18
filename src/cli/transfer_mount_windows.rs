//! Windows mounts name volumes, not image files or physical drives. A
//! volume's disk extents place it on physical drives, and an attached
//! virtual disk exposes its backing file as one of those drives. This module
//! decides the mount state from that snapshot without calling any Windows
//! API, so every host compiles and tests it. Backing files compare by file
//! identity, not path spelling, as the Unix probes do.
use std::collections::BTreeSet;
use std::io;

#[derive(Default, Debug, PartialEq)]
pub(super) struct MountState {
    pub mounted: bool,
    pub read_write: bool,
}

/// A transfer target named in the Windows device namespace.
#[derive(Debug, PartialEq)]
pub(super) enum Device {
    /// `\\.\PhysicalDriveN`: every volume with an extent on drive N.
    Drive(u32),
    /// `\\.\C:` or `\\?\Volume{GUID}`: that volume alone, as the root path
    /// the volume APIs take (`C:\` or `\\?\Volume{GUID}\`).
    Volume(String),
}

/// One volume of the host snapshot.
#[derive(Debug)]
pub(super) struct Volume {
    /// The `\\?\Volume{GUID}\` name.
    pub name: String,
    /// Physical drive numbers holding the volume's extents.
    pub drives: Vec<u32>,
    /// Drive letters and folder mount points; empty when unmounted.
    pub mount_points: Vec<String>,
    pub read_only: bool,
}

/// An attached virtual disk: the identity of its backing file and the
/// physical drive number it is exposed as.
#[derive(Debug)]
pub(super) struct Attached<I> {
    pub backing: I,
    pub drive: u32,
}

/// The target after classification: a file by identity, a drive, or a
/// volume by its `\\?\Volume{GUID}\` name.
#[derive(Debug)]
pub(super) enum Target<'a, I> {
    File(&'a I),
    Drive(u32),
    Volume(&'a str),
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Classifies a target path. `Ok(None)` is a file path, including
/// `\\?\C:\...` and `\\?\UNC\...`; a device-namespace path that is neither a
/// drive nor a volume is an error, so the probe fails closed.
pub(super) fn device(path: &str) -> io::Result<Option<Device>> {
    let normal = path.replace('/', "\\");
    let (rest, verbatim) = if let Some(rest) = normal.strip_prefix(r"\\.\") {
        (rest, false)
    } else if let Some(rest) = normal.strip_prefix(r"\\?\") {
        (rest, true)
    } else {
        return Ok(None);
    };
    let unknown = || invalid(format!("unrecognised device path {path}"));
    if let Some(number) = strip_prefix_ignore_case(rest, "PhysicalDrive") {
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(unknown());
        }
        return number
            .parse()
            .map(|number| Some(Device::Drive(number)))
            .map_err(|_| unknown());
    }
    let bytes = rest.as_bytes();
    let (root, tail) = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        (format!(r"{}:\", rest[..1].to_ascii_uppercase()), &rest[2..])
    } else if let Some(guid) = strip_prefix_ignore_case(rest, "Volume{") {
        let (guid, tail) = guid.split_once('}').ok_or_else(unknown)?;
        (format!(r"\\?\Volume{{{guid}}}\"), tail)
    } else if verbatim && strip_prefix_ignore_case(rest, r"UNC\").is_some() {
        return Ok(None);
    } else {
        return Err(unknown());
    };
    if tail.is_empty() || tail == r"\" {
        Ok(Some(Device::Volume(root)))
    } else if verbatim {
        // A verbatim path below a volume root names a file on it.
        Ok(None)
    } else {
        Err(unknown())
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// Decides the mount state of `target` from the attached virtual disks and
/// the volumes. A whole drive or image is mounted when any volume with an
/// extent on it has a mount point; a volume target only by its own.
pub(super) fn decide<I: PartialEq>(
    target: Target<'_, I>,
    attached: &[Attached<I>],
    volumes: &[Volume],
) -> io::Result<MountState> {
    let drives: BTreeSet<u32> = match target {
        Target::Volume(name) => {
            let volume = volumes
                .iter()
                .find(|volume| volume.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| invalid(format!("volume {name} is not in the host volume list")))?;
            let mounted = !volume.mount_points.is_empty();
            return Ok(MountState {
                mounted,
                read_write: mounted && !volume.read_only,
            });
        }
        Target::Drive(drive) => BTreeSet::from([drive]),
        Target::File(identity) => attached
            .iter()
            .filter(|disk| disk.backing == *identity)
            .map(|disk| disk.drive)
            .collect(),
    };
    let mut state = MountState::default();
    if drives.is_empty() {
        return Ok(state);
    }
    for volume in volumes
        .iter()
        .filter(|volume| !volume.mount_points.is_empty())
    {
        // Without extents the volume could sit on the target: fail closed.
        if volume.drives.is_empty() {
            return Err(invalid(format!(
                "mounted volume {} reports no disk extents",
                volume.name
            )));
        }
        if volume.drives.iter().any(|drive| drives.contains(drive)) {
            state.mounted = true;
            state.read_write |= !volume.read_only;
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(name: &str, drives: &[u32], mount_points: &[&str], read_only: bool) -> Volume {
        Volume {
            name: name.to_owned(),
            drives: drives.to_vec(),
            mount_points: mount_points
                .iter()
                .map(|point| (*point).to_owned())
                .collect(),
            read_only,
        }
    }

    const A: &str = r"\\?\Volume{0a0a0a0a-0000-0000-0000-00000000000a}\";
    const B: &str = r"\\?\Volume{0b0b0b0b-0000-0000-0000-00000000000b}\";

    #[test]
    fn device_paths_classify_drives_volumes_and_files() {
        for (path, expected) in [
            (r"\\.\PhysicalDrive3", Some(Device::Drive(3))),
            (r"\\.\physicaldrive12", Some(Device::Drive(12))),
            (r"\\?\PhysicalDrive0", Some(Device::Drive(0))),
            (r"\\.\E:", Some(Device::Volume(r"E:\".to_owned()))),
            (r"\\.\e:\", Some(Device::Volume(r"E:\".to_owned()))),
            (
                r"\\?\Volume{0a0a0a0a-0000-0000-0000-00000000000a}",
                Some(Device::Volume(A.to_owned())),
            ),
            (A, Some(Device::Volume(A.to_owned()))),
            (r"C:\images\disk.vhdx", None),
            (r"disk.img", None),
            (r"\\?\C:\images\disk.vhdx", None),
            (r"\\?\UNC\server\share\disk.vhd", None),
            (
                r"\\?\Volume{0a0a0a0a-0000-0000-0000-00000000000a}\images\disk.vhd",
                None,
            ),
            (r"\\server\share\disk.vhd", None),
            ("//./PhysicalDrive2", Some(Device::Drive(2))),
        ] {
            assert_eq!(device(path).unwrap(), expected, "{path}");
        }
        for path in [
            r"\\.\PhysicalDrive",
            r"\\.\PhysicalDrive1x",
            r"\\.\PhysicalDrive-1",
            r"\\.\HarddiskVolume3",
            r"\\.\CdRom0",
            r"\\?\GLOBALROOT\Device\Harddisk0\Partition1",
            r"\\.\C:\file",
        ] {
            assert!(device(path).is_err(), "{path} must fail closed");
        }
    }

    #[test]
    fn images_match_attached_drives_by_identity() {
        let attached = [
            Attached {
                backing: 7u64,
                drive: 2,
            },
            Attached {
                backing: 9u64,
                drive: 3,
            },
        ];
        let volumes = [
            volume(A, &[2], &[r"F:\"], false),
            volume(B, &[3], &[], false),
        ];
        assert_eq!(
            decide(Target::File(&7), &attached, &volumes).unwrap(),
            MountState {
                mounted: true,
                read_write: true
            }
        );
        // Attached but with no mounted volume, and not attached at all.
        assert_eq!(
            decide(Target::File(&9), &attached, &volumes).unwrap(),
            MountState::default()
        );
        assert_eq!(
            decide(Target::File(&8), &attached, &volumes).unwrap(),
            MountState::default()
        );
    }

    #[test]
    fn drives_combine_every_volume_and_honour_read_only() {
        let volumes = [
            volume(A, &[1], &[r"G:\"], true),
            volume(B, &[1, 4], &[r"C:\mnt\b\"], false),
        ];
        assert_eq!(
            decide(Target::<u64>::Drive(1), &[], &volumes[..1]).unwrap(),
            MountState {
                mounted: true,
                read_write: false
            }
        );
        assert_eq!(
            decide(Target::<u64>::Drive(1), &[], &volumes).unwrap(),
            MountState {
                mounted: true,
                read_write: true
            }
        );
        // A spanned volume mounts every drive it has an extent on.
        assert!(
            decide(Target::<u64>::Drive(4), &[], &volumes)
                .unwrap()
                .mounted
        );
        assert_eq!(
            decide(Target::<u64>::Drive(5), &[], &volumes).unwrap(),
            MountState::default()
        );
    }

    #[test]
    fn volumes_decide_alone_by_case_insensitive_name() {
        let volumes = [
            volume(A, &[1], &[r"G:\"], true),
            volume(B, &[1], &[], false),
        ];
        let upper = A.to_ascii_uppercase();
        assert_eq!(
            decide(Target::<u64>::Volume(&upper), &[], &volumes).unwrap(),
            MountState {
                mounted: true,
                read_write: false
            }
        );
        // A sibling volume on the same drive does not mount this one.
        assert_eq!(
            decide(Target::<u64>::Volume(B), &[], &volumes).unwrap(),
            MountState::default()
        );
    }

    #[test]
    fn unresolvable_snapshots_fail_closed() {
        let missing = r"\\?\Volume{0c0c0c0c-0000-0000-0000-00000000000c}\";
        let volumes = [volume(A, &[1], &[r"G:\"], false)];
        assert!(decide(Target::<u64>::Volume(missing), &[], &volumes).is_err());
        // A mounted volume without extents could sit on any drive.
        let unplaced = [volume(B, &[], &[r"H:\"], false)];
        let attached = [Attached {
            backing: 7u64,
            drive: 2,
        }];
        assert!(decide(Target::<u64>::Drive(1), &[], &unplaced).is_err());
        assert!(decide(Target::File(&7), &attached, &unplaced).is_err());
        // An image attached nowhere has no drive for it to sit on.
        assert_eq!(
            decide(Target::File(&8), &attached, &unplaced).unwrap(),
            MountState::default()
        );
        // An unmounted one cannot matter.
        let idle = [volume(B, &[], &[], false)];
        assert_eq!(
            decide(Target::<u64>::Drive(1), &[], &idle).unwrap(),
            MountState::default()
        );
    }
}
