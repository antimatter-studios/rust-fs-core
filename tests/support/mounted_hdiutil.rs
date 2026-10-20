//! Real macOS DiskImages attachments are the oracle; no missing prerequisite is a pass.
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(tool: &str, args: &[&OsStr]) -> Output {
    let output = Command::new(tool)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("macOS mount oracle requires {tool}: {err}"));
    assert!(
        output.status.success(),
        "macOS mount oracle requires {tool}: {output:?}"
    );
    output
}

/// Attaching images changes the host, so the oracle runs on disposable CI
/// hosts. Elsewhere it fails unless the unprivileged skip is acknowledged.
pub fn host() -> bool {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        return true;
    }
    assert!(
        std::env::var_os("AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP").is_some(),
        "macOS mount oracle requires a disposable CI host (GITHUB_ACTIONS)"
    );
    eprintln!("skipped: macOS mount oracle attaches images only on CI hosts");
    false
}

pub struct Volume {
    pub image: PathBuf,
    pub device: PathBuf,
    filesystem: PathBuf,
    mount: PathBuf,
    mounted: bool,
    attached: bool,
}
impl Volume {
    pub fn new(root: &Path, partitioned: bool) -> Self {
        let image = root.join("volume.dmg");
        let layout = if partitioned { "MBRSPUD" } else { "NONE" };
        run(
            "hdiutil",
            &[
                "create".as_ref(),
                "-quiet".as_ref(),
                "-size".as_ref(),
                "16m".as_ref(),
                "-fs".as_ref(),
                "HFS+".as_ref(),
                "-volname".as_ref(),
                "transfer".as_ref(),
                "-layout".as_ref(),
                layout.as_ref(),
                image.as_os_str(),
            ],
        );
        let output = run(
            "hdiutil",
            &[
                "attach".as_ref(),
                "-nobrowse".as_ref(),
                "-nomount".as_ref(),
                image.as_os_str(),
            ],
        );
        // One tab-separated line per attached node: whole disk first.
        let nodes: Vec<PathBuf> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|node| node.starts_with("/dev/disk"))
            .map(PathBuf::from)
            .collect();
        assert!(!nodes.is_empty(), "hdiutil attach must report a device");
        let device = nodes[0].clone();
        let filesystem = nodes[nodes.len() - 1].clone();
        let mount = root.join("mount");
        fs::create_dir(&mount).unwrap();
        let mut volume = Self {
            image,
            device,
            filesystem,
            mount,
            mounted: false,
            attached: true,
        };
        // DiskImages gives the attaching user the node; no chmod is needed.
        volume.attach_mount(false);
        volume
    }
    fn attach_mount(&mut self, readonly: bool) {
        let mut args: Vec<&OsStr> = vec!["mount".as_ref()];
        if readonly {
            args.push("readOnly".as_ref());
        }
        args.extend([
            "-mountPoint".as_ref(),
            self.mount.as_os_str(),
            self.filesystem.as_os_str(),
        ]);
        run("diskutil", &args);
        self.mounted = true;
    }
    pub fn remount(&mut self, readonly: bool) {
        self.unmount();
        self.attach_mount(readonly);
    }
    pub fn unmount(&mut self) {
        run("diskutil", &["unmount".as_ref(), self.mount.as_os_str()]);
        self.mounted = false;
    }
    pub fn detach(&mut self) {
        if self.mounted {
            self.unmount();
        }
        let detached = Command::new("hdiutil")
            .args(["detach"])
            .arg(&self.device)
            .output()
            .unwrap_or_else(|err| panic!("macOS mount oracle requires hdiutil: {err}"));
        if !detached.status.success() {
            let stderr = String::from_utf8_lossy(&detached.stderr);
            assert!(
                stderr.to_ascii_lowercase().contains("resource busy"),
                "macOS mount oracle requires hdiutil: {detached:?}"
            );
            // Disk Arbitration can still be releasing the just-unmounted
            // partition. This image is disposable and all test children have
            // exited, so force detachment to avoid leaking the device.
            run(
                "hdiutil",
                &[
                    "detach".as_ref(),
                    "-force".as_ref(),
                    self.device.as_os_str(),
                ],
            );
        }
        self.attached = false;
    }
}
impl Drop for Volume {
    fn drop(&mut self) {
        if !self.attached {
            return;
        }
        // During a failed assertion, a second panic would abort before the
        // image detaches; force it and leave the first failure to report.
        if std::thread::panicking() {
            let _ = Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(&self.device)
                .output();
            return;
        }
        self.detach();
    }
}
