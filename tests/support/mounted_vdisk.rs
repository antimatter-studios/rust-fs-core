//! Real Windows virtual-disk attachments are the oracle; no missing prerequisite is a pass.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(tool: &str, args: &[&str]) -> Output {
    let output = Command::new(tool)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("Windows mount oracle requires {tool}: {err}"));
    assert!(
        output.status.success(),
        "Windows mount oracle requires {tool} and administrator rights: {output:?}"
    );
    output
}

fn powershell(script: &str) -> String {
    let output = run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("$ErrorActionPreference = 'Stop'; {script}"),
        ],
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Attaching disks changes the host, so the oracle runs on disposable CI
/// hosts. Elsewhere it fails unless the unprivileged skip is acknowledged.
pub fn host() -> bool {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        return true;
    }
    assert!(
        std::env::var_os("AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP").is_some(),
        "Windows mount oracle requires a disposable CI host (GITHUB_ACTIONS)"
    );
    eprintln!("skipped: Windows mount oracle attaches disks only on CI hosts");
    false
}

/// An NTFS volume on a VHD, attached and mounted at a drive letter.
pub struct Volume {
    pub image: PathBuf,
    pub drive: PathBuf,
    pub volume: PathBuf,
    attached: bool,
}
impl Volume {
    pub fn new(root: &Path) -> Self {
        let image = root.join("volume.vhd");
        let script = root.join("create.txt");
        let file = image.display();
        fs::write(
            &script,
            format!(
                "create vdisk file=\"{file}\" maximum=32 type=fixed\n\
                 select vdisk file=\"{file}\"\n\
                 attach vdisk\n\
                 create partition primary\n\
                 format fs=ntfs quick label=transfer\n\
                 assign\n\
                 detach vdisk\n"
            ),
        )
        .unwrap();
        run("diskpart", &["/s", &script.display().to_string()]);
        let mut volume = Self {
            image,
            drive: PathBuf::new(),
            volume: PathBuf::new(),
            attached: false,
        };
        volume.attach(false);
        volume
    }
    fn quoted(&self) -> String {
        format!("'{}'", self.image.display().to_string().replace('\'', "''"))
    }
    pub fn attach(&mut self, readonly: bool) {
        let access = if readonly { "ReadOnly" } else { "ReadWrite" };
        let image = self.quoted();
        // Windows mounts the partition and gives it the next free letter.
        let output = powershell(&format!(
            "Mount-DiskImage -ImagePath {image} -StorageType VHD -Access {access} | Out-Null; \
             $disk = Get-DiskImage -ImagePath {image}; \
             $letter = ($disk | Get-Disk | Get-Partition | Get-Volume).DriveLetter; \
             Write-Output \"$($disk.Number) $letter\""
        ));
        self.attached = true;
        let mut fields = output.split_whitespace();
        let number = fields
            .next()
            .expect("Mount-DiskImage must report a disk number");
        let letter = fields
            .next()
            .expect("Windows must mount the attached volume at a drive letter");
        self.drive = PathBuf::from(format!(r"\\.\PhysicalDrive{number}"));
        self.volume = PathBuf::from(format!(r"\\.\{letter}:"));
    }
    pub fn reattach(&mut self, readonly: bool) {
        self.detach();
        self.attach(readonly);
    }
    pub fn detach(&mut self) {
        powershell(&format!(
            "Dismount-DiskImage -ImagePath {} | Out-Null",
            self.quoted()
        ));
        self.attached = false;
    }
}
impl Drop for Volume {
    fn drop(&mut self) {
        if !self.attached {
            return;
        }
        // During a failed assertion, a second panic would abort before the
        // disk detaches; try once and leave the first failure to report.
        if std::thread::panicking() {
            let _ = Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!("Dismount-DiskImage -ImagePath {}", self.quoted()))
                .output();
            return;
        }
        self.detach();
    }
}
