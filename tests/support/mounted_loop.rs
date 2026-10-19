//! Real Linux kernel mounts are the oracle; no missing prerequisite is a pass.
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn run(tool: &str, args: &[&OsStr]) -> Output {
    let output = Command::new("sudo")
        .args(["-n", tool])
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("Linux mount oracle requires sudo -n {tool}: {err}"));
    assert!(
        output.status.success(),
        "Linux mount oracle requires {tool}: {output:?}"
    );
    output
}

pub struct Volume {
    pub device: PathBuf,
    pub filesystem: PathBuf,
    mount: PathBuf,
    mounted: bool,
    original_mode: u32,
}
impl Volume {
    pub fn new(root: &Path, image: &Path, partitioned: bool) -> Self {
        File::options()
            .write(true)
            .open(image)
            .unwrap()
            .set_len(16 * 1024 * 1024)
            .unwrap();
        if partitioned {
            let mut child = Command::new("sudo")
                .args(["-n", "sfdisk"])
                .arg(image)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .expect("Linux partition oracle requires sudo -n sfdisk (fdisk package)");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"label: dos\n\nstart=2048, type=83\n")
                .unwrap();
            assert!(
                child.wait().unwrap().success(),
                "sfdisk must create the partition oracle"
            );
        }
        let output = run(
            "losetup",
            &[
                "--find".as_ref(),
                "--show".as_ref(),
                "--partscan".as_ref(),
                image.as_os_str(),
            ],
        );
        let device = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let filesystem = if partitioned {
            PathBuf::from(format!("{}p1", device.display()))
        } else {
            device.clone()
        };
        let mount = root.join("mount");
        fs::create_dir(&mount).unwrap();
        let mut volume = Self {
            original_mode: fs::metadata(&device).unwrap().permissions().mode(),
            device,
            filesystem,
            mount,
            mounted: false,
        };
        // Only our newly allocated loop node; restore its mode before detach.
        run("udevadm", &["settle".as_ref()]);
        run("chmod", &["0666".as_ref(), volume.device.as_os_str()]);
        run(
            "mkfs.ext4",
            &["-q".as_ref(), "-F".as_ref(), volume.filesystem.as_os_str()],
        );
        run(
            "mount",
            &[volume.filesystem.as_os_str(), volume.mount.as_os_str()],
        );
        volume.mounted = true;
        volume
    }
    pub fn remount(&mut self, readonly: bool) {
        run(
            "mount",
            &[
                "-o".as_ref(),
                if readonly { "remount,ro" } else { "remount,rw" }.as_ref(),
                self.mount.as_os_str(),
            ],
        );
    }
    pub fn unmount(&mut self) {
        run("umount", &[self.mount.as_os_str()]);
        self.mounted = false;
    }
}
impl Drop for Volume {
    fn drop(&mut self) {
        if self.mounted {
            run("umount", &[self.mount.as_os_str()]);
        }
        run(
            "chmod",
            &[
                format!("{:o}", self.original_mode & 0o7777).as_ref(),
                self.device.as_os_str(),
            ],
        );
        run("losetup", &["--detach".as_ref(), self.device.as_os_str()]);
        run("udevadm", &["settle".as_ref()]);
    }
}

fn attach(image: &Path, size: u64) -> (PathBuf, u32) {
    File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(image)
        .unwrap()
        .set_len(size)
        .unwrap();
    let output = run(
        "losetup",
        &["--find".as_ref(), "--show".as_ref(), image.as_os_str()],
    );
    let device = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    let mode = fs::metadata(&device).unwrap().permissions().mode();
    run("udevadm", &["settle".as_ref()]);
    run("chmod", &["0666".as_ref(), device.as_os_str()]);
    (device, mode)
}

fn detach(device: &Path, mode: u32) {
    run(
        "chmod",
        &[format!("{:o}", mode & 0o7777).as_ref(), device.as_os_str()],
    );
    run("losetup", &["--detach".as_ref(), device.as_os_str()]);
    run("udevadm", &["settle".as_ref()]);
}

/// An attached loop device that no filesystem uses.
pub struct Loop {
    pub device: PathBuf,
    mode: u32,
}
impl Loop {
    pub fn new(image: &Path) -> Self {
        let (device, mode) = attach(image, 16 * 1024 * 1024);
        Self { device, mode }
    }
}
impl Drop for Loop {
    fn drop(&mut self) {
        detach(&self.device, self.mode);
    }
}

/// One Btrfs filesystem spanning a loop device per image; the kernel reports
/// it under a synthetic device ID and names only one member as its source.
pub struct Btrfs {
    pub devices: Vec<PathBuf>,
    modes: Vec<u32>,
    mount: PathBuf,
    mounted: bool,
}
impl Btrfs {
    pub fn new(root: &Path, images: &[PathBuf]) -> Self {
        // Sparse; above mkfs.btrfs's per-device minimum for the default profile.
        let (devices, modes): (Vec<_>, Vec<_>) = images
            .iter()
            .map(|image| attach(image, 256 * 1024 * 1024))
            .unzip();
        let mut args: Vec<&OsStr> = vec!["-q".as_ref(), "-f".as_ref()];
        args.extend(devices.iter().map(|device| device.as_os_str()));
        run("mkfs.btrfs", &args);
        run("udevadm", &["settle".as_ref()]);
        let mount = root.join("btrfs-mount");
        fs::create_dir(&mount).unwrap();
        run("mount", &[devices[0].as_os_str(), mount.as_os_str()]);
        Self {
            devices,
            modes,
            mount,
            mounted: true,
        }
    }
    pub fn remount(&mut self, readonly: bool) {
        run(
            "mount",
            &[
                "-o".as_ref(),
                if readonly { "remount,ro" } else { "remount,rw" }.as_ref(),
                self.mount.as_os_str(),
            ],
        );
    }
    pub fn unmount(&mut self) {
        run("umount", &[self.mount.as_os_str()]);
        self.mounted = false;
    }
}
impl Drop for Btrfs {
    fn drop(&mut self) {
        if self.mounted {
            run("umount", &[self.mount.as_os_str()]);
        }
        for (device, mode) in self.devices.iter().zip(&self.modes) {
            detach(device, *mode);
        }
    }
}

/// One ZFS pool striped over a loop device per image, its root dataset
/// mounted; the kernel reports the dataset by pool name, not by any member.
pub struct Zfs {
    pub devices: Vec<PathBuf>,
    modes: Vec<u32>,
    pool: String,
    imported: bool,
}
impl Zfs {
    pub fn new(root: &Path, images: &[PathBuf]) -> Self {
        // Sparse; above zpool's 64 MiB per-vdev minimum.
        let (devices, modes): (Vec<_>, Vec<_>) = images
            .iter()
            .map(|image| attach(image, 128 * 1024 * 1024))
            .unzip();
        let pool = format!("fscore{}", std::process::id());
        let mount = root.join("zfs-mount");
        let mut args: Vec<&OsStr> = vec![
            "create".as_ref(),
            "-f".as_ref(),
            "-m".as_ref(),
            mount.as_os_str(),
            pool.as_ref(),
        ];
        args.extend(devices.iter().map(|device| device.as_os_str()));
        run("zpool", &args);
        run("udevadm", &["settle".as_ref()]);
        Self {
            devices,
            modes,
            pool,
            imported: true,
        }
    }
    pub fn set_readonly(&mut self, readonly: bool) {
        let property = format!("readonly={}", if readonly { "on" } else { "off" });
        run(
            "zfs",
            &["set".as_ref(), property.as_ref(), self.pool.as_ref()],
        );
    }
    pub fn export(&mut self) {
        run("zpool", &["export".as_ref(), self.pool.as_ref()]);
        run("udevadm", &["settle".as_ref()]);
        self.imported = false;
    }
}
impl Drop for Zfs {
    fn drop(&mut self) {
        if self.imported {
            run(
                "zpool",
                &["destroy".as_ref(), "-f".as_ref(), self.pool.as_ref()],
            );
        }
        for (device, mode) in self.devices.iter().zip(&self.modes) {
            detach(device, *mode);
        }
    }
}
