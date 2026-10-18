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
