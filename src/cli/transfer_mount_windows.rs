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

/// Snapshots the host's volumes and attached virtual disks and decides the
/// mount state of `image`. Any failed query fails closed, naming the query.
#[cfg(windows)]
pub(super) fn state(image: &std::path::Path) -> Result<MountState, super::CliError> {
    probe::state(image).map_err(|err| {
        super::CliError::failed(format!(
            "cannot check Windows mounts of {}: {err}",
            image.display()
        ))
    })
}

#[cfg(windows)]
mod probe {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr::null_mut;

    use super::{decide, device, Attached, Device, MountState, Target, Volume};

    /// A file by volume serial number and file index.
    type Identity = (u32, u64);
    type Handle = *mut c_void;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const ERROR_NO_MORE_FILES: i32 = 18;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ERROR_MORE_DATA: i32 = 234;
    const ERROR_VIRTDISK_NOT_VIRTUAL_DISK: u32 = 0xC03A_0015;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES: u32 = 0x0000_0001;
    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    const SHARE_ALL: u32 = 7;
    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
    const STORAGE_DEPENDENCY_INFO_VERSION_2: u64 = 2;
    const NAME_LENGTH: usize = 260;

    #[repr(C)]
    #[derive(Default)]
    struct FileInformation {
        attributes: u32,
        times: [u32; 6],
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }

    /// STORAGE_DEPENDENCY_INFO_TYPE_2.
    #[repr(C)]
    struct Dependency {
        type_flags: u32,
        provider_flags: u32,
        device_id: u32,
        vendor_id: [u8; 16],
        ancestor_level: u32,
        device_name: *const u16,
        host_volume_name: *const u16,
        dependent_volume_name: *const u16,
        dependent_volume_relative_path: *const u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn FindFirstVolumeW(name: *mut u16, length: u32) -> Handle;
        fn FindNextVolumeW(find: Handle, name: *mut u16, length: u32) -> i32;
        fn FindVolumeClose(find: Handle) -> i32;
        fn GetVolumePathNamesForVolumeNameW(
            volume: *const u16,
            names: *mut u16,
            length: u32,
            needed: *mut u32,
        ) -> i32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_length: u32,
            serial: *mut u32,
            component_length: *mut u32,
            flags: *mut u32,
            file_system: *mut u16,
            file_system_length: u32,
        ) -> i32;
        fn GetVolumeNameForVolumeMountPointW(point: *const u16, name: *mut u16, length: u32)
            -> i32;
        fn DeviceIoControl(
            device: Handle,
            code: u32,
            input: *mut c_void,
            input_length: u32,
            output: *mut c_void,
            output_length: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn GetFileInformationByHandle(file: Handle, information: *mut FileInformation) -> i32;
    }

    #[link(name = "virtdisk")]
    extern "system" {
        fn GetStorageDependencyInformation(
            object: Handle,
            flags: u32,
            size: u32,
            information: *mut c_void,
            used: *mut u32,
        ) -> u32;
    }

    pub(super) fn state(image: &Path) -> io::Result<MountState> {
        let target = match image.to_str().map(device).transpose()?.flatten() {
            Some(Device::Drive(drive)) => {
                return decide(Target::<Identity>::Drive(drive), &[], &volumes()?);
            }
            Some(Device::Volume(root)) => {
                let name = if root.starts_with(r"\\?\") {
                    root
                } else {
                    volume_name(&root)?
                };
                return decide(Target::<Identity>::Volume(&name), &[], &volumes()?);
            }
            None => identity(&open_query(image)?, image)?,
        };
        let volumes = volumes()?;
        let attached = attached(&volumes)?;
        decide(Target::File(&target), &attached, &volumes)
    }

    fn wide(text: &str) -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    fn until_nul(buffer: &[u16]) -> String {
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }

    fn failed(query: String, err: io::Error) -> io::Error {
        io::Error::new(err.kind(), format!("{query}: {err}"))
    }

    fn last_failed(query: String) -> io::Error {
        failed(query, io::Error::last_os_error())
    }

    /// A handle that answers queries only: no read or write access.
    fn open_query(path: impl AsRef<Path>) -> io::Result<File> {
        let path = path.as_ref();
        OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(SHARE_ALL)
            .open(path)
            .map_err(|err| failed(format!("CreateFileW {}", path.display()), err))
    }

    fn identity(file: &File, path: &Path) -> io::Result<Identity> {
        let mut information = FileInformation::default();
        // SAFETY: the live File supplies a valid handle and the C-layout
        // structure is writable for the duration of the call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(last_failed(format!(
                "GetFileInformationByHandle {}",
                path.display()
            )));
        }
        let index = u64::from(information.index_high) << 32 | u64::from(information.index_low);
        Ok((information.volume_serial, index))
    }

    fn volume_name(root: &str) -> io::Result<String> {
        let point = wide(root);
        let mut name = [0u16; NAME_LENGTH];
        // SAFETY: both buffers are NUL-terminated or sized as passed.
        if unsafe {
            GetVolumeNameForVolumeMountPointW(point.as_ptr(), name.as_mut_ptr(), NAME_LENGTH as u32)
        } == 0
        {
            return Err(last_failed(format!(
                "GetVolumeNameForVolumeMountPointW {root}"
            )));
        }
        Ok(until_nul(&name))
    }

    fn volumes() -> io::Result<Vec<Volume>> {
        let mut name = [0u16; NAME_LENGTH];
        // SAFETY: the buffer is writable for the length passed.
        let find = unsafe { FindFirstVolumeW(name.as_mut_ptr(), NAME_LENGTH as u32) };
        if find == INVALID_HANDLE_VALUE {
            return Err(last_failed("FindFirstVolumeW".to_owned()));
        }
        let mut names = Vec::new();
        let listed = loop {
            names.push(until_nul(&name));
            // SAFETY: `find` is the live search handle from FindFirstVolumeW.
            if unsafe { FindNextVolumeW(find, name.as_mut_ptr(), NAME_LENGTH as u32) } == 0 {
                let err = io::Error::last_os_error();
                break if err.raw_os_error() == Some(ERROR_NO_MORE_FILES) {
                    Ok(())
                } else {
                    Err(failed("FindNextVolumeW".to_owned(), err))
                };
            }
        };
        // SAFETY: closes the search handle exactly once.
        unsafe { FindVolumeClose(find) };
        listed?;
        names.into_iter().map(volume).collect()
    }

    /// Only a mounted volume can mount a target, so only those are placed
    /// on drives and checked for the read-only flag.
    fn volume(name: String) -> io::Result<Volume> {
        let mount_points = mount_points(&name)?;
        let (drives, read_only) = if mount_points.is_empty() {
            (Vec::new(), false)
        } else {
            (drives(&name)?, read_only(&name)?)
        };
        Ok(Volume {
            name,
            drives,
            mount_points,
            read_only,
        })
    }

    fn mount_points(volume: &str) -> io::Result<Vec<String>> {
        let name = wide(volume);
        let mut buffer = vec![0u16; NAME_LENGTH];
        loop {
            let mut needed = 0u32;
            // SAFETY: the name is NUL-terminated and the buffer is writable
            // for the length passed.
            if unsafe {
                GetVolumePathNamesForVolumeNameW(
                    name.as_ptr(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut needed,
                )
            } != 0
            {
                return Ok(buffer
                    .split(|&unit| unit == 0)
                    .take_while(|point| !point.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect());
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_MORE_DATA) {
                return Err(failed(
                    format!("GetVolumePathNamesForVolumeNameW {volume}"),
                    err,
                ));
            }
            let grown = (needed as usize).max(buffer.len() * 2);
            buffer.resize(grown, 0);
        }
    }

    fn read_only(volume: &str) -> io::Result<bool> {
        let root = wide(volume);
        let mut flags = 0u32;
        // SAFETY: the root is NUL-terminated; every other output is null
        // with a zero length, as the API allows.
        if unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut flags,
                null_mut(),
                0,
            )
        } == 0
        {
            return Err(last_failed(format!("GetVolumeInformationW {volume}")));
        }
        Ok(flags & FILE_READ_ONLY_VOLUME != 0)
    }

    /// The volume device: its `\\?\Volume{GUID}` name without the slash.
    fn volume_device(volume: &str) -> io::Result<File> {
        open_query(volume.trim_end_matches('\\'))
    }

    fn drives(volume: &str) -> io::Result<Vec<u32>> {
        let device = volume_device(volume)?;
        // VOLUME_DISK_EXTENTS: a count, then 24-byte DISK_EXTENTs from
        // offset 8, each opening with its DiskNumber.
        let mut buffer = vec![0u64; 1 + 3 * 8];
        loop {
            let mut returned = 0u32;
            // SAFETY: the u64 buffer is aligned for the structure and
            // writable for the byte length passed.
            if unsafe {
                DeviceIoControl(
                    device.as_raw_handle(),
                    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                    null_mut(),
                    0,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                    &mut returned,
                    null_mut(),
                )
            } != 0
            {
                let count = (buffer[0] as u32 as usize).min((buffer.len() - 1) / 3);
                return Ok((0..count)
                    .map(|index| buffer[1 + 3 * index] as u32)
                    .collect());
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_MORE_DATA) {
                return Err(failed(
                    format!("IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS {volume}"),
                    err,
                ));
            }
            let count = (buffer[0] as u32 as usize).max((buffer.len() - 1) / 3 * 2);
            buffer.resize(1 + 3 * count, 0);
        }
    }

    /// Every virtual disk a mounted volume sits on, by backing-file
    /// identity, exposed as each of the volume's drives.
    fn attached(volumes: &[Volume]) -> io::Result<Vec<Attached<Identity>>> {
        let mut attached = Vec::new();
        for volume in volumes
            .iter()
            .filter(|volume| !volume.mount_points.is_empty())
        {
            for backing in backing_files(&volume.name)? {
                attached.extend(
                    volume
                        .drives
                        .iter()
                        .map(|&drive| Attached { backing, drive }),
                );
            }
        }
        Ok(attached)
    }

    fn backing_files(volume: &str) -> io::Result<Vec<Identity>> {
        let device = volume_device(volume)?;
        let query = || format!("GetStorageDependencyInformation {volume}");
        let mut buffer = vec![0u64; 64];
        loop {
            // STORAGE_DEPENDENCY_INFO: Version, then NumberEntries.
            buffer[0] = STORAGE_DEPENDENCY_INFO_VERSION_2;
            let size = (buffer.len() * 8) as u32;
            let mut used = 0u32;
            // SAFETY: the u64 buffer is aligned for the structure and
            // writable for the byte length passed.
            let status = unsafe {
                GetStorageDependencyInformation(
                    device.as_raw_handle(),
                    GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES,
                    size,
                    buffer.as_mut_ptr().cast(),
                    &mut used,
                )
            };
            match status {
                0 => break,
                ERROR_VIRTDISK_NOT_VIRTUAL_DISK => return Ok(Vec::new()),
                ERROR_INSUFFICIENT_BUFFER if used > size => {
                    buffer = vec![0u64; (used as usize).div_ceil(8)];
                }
                _ => return Err(failed(query(), io::Error::from_raw_os_error(status as i32))),
            }
        }
        let capacity = (buffer.len() - 1) * 8 / std::mem::size_of::<Dependency>();
        let count = ((buffer[0] >> 32) as usize).min(capacity);
        // SAFETY: the entries start at offset 8, are aligned by the u64
        // buffer, and the count is bounded by the buffer's size.
        let entries = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().add(1).cast::<Dependency>(), count)
        };
        entries
            .iter()
            .map(|entry| {
                // SAFETY: the API points these strings into `buffer`, which
                // is alive here.
                let (host, relative) = unsafe {
                    (
                        terminated(entry.host_volume_name),
                        terminated(entry.dependent_volume_relative_path),
                    )
                };
                let (Some(host), Some(relative)) = (host, relative) else {
                    return Err(failed(
                        query(),
                        io::Error::new(io::ErrorKind::InvalidData, "a backing file has no path"),
                    ));
                };
                let separator = if host.ends_with('\\') { "" } else { "\\" };
                let backing = format!("{host}{separator}{relative}");
                identity(&open_query(&backing)?, Path::new(&backing))
            })
            .collect()
    }

    /// # Safety
    ///
    /// `text` is null or points to a live NUL-terminated UTF-16 string.
    unsafe fn terminated(text: *const u16) -> Option<String> {
        if text.is_null() {
            return None;
        }
        let mut length = 0;
        // SAFETY: the caller guarantees a NUL terminator.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` units precede the terminator.
        Some(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, length)
        }))
    }
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
