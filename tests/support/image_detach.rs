//! Support for DiskImages tests: detach by node, and an owned attachment
//! that detaches on drop. Each test crate includes this file and uses a subset.
#![allow(dead_code)]
use std::ffi::OsStr;
use std::process::Command;
use std::time::Duration;

/// Detach attempts before a busy node is reported as still attached.
pub const ATTEMPTS: u32 = 6;

/// Detach `device`, retrying while it reports "Resource busy" (a scanning app
/// holds the node for a moment). Any other failure, or a node still busy after
/// the last attempt, returns the tool's stderr.
pub fn detach(program: &OsStr, device: &OsStr) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        let mut command = Command::new(program);
        command.arg("detach");
        if attempt > 1 {
            command.arg("-force");
        }
        let output = command
            .arg(device)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !stderr.to_ascii_lowercase().contains("resource busy") || attempt == ATTEMPTS {
            return Err(stderr);
        }
        std::thread::sleep(Duration::from_millis(200 * u64::from(attempt)));
    }
    unreachable!("the loop returns on its last attempt")
}

/// Detach what one attach reported: its whole-disk `device` node when attach
/// named one, otherwise the `mount` point, which detaches the same image.
pub fn detach_attached(program: &OsStr, device: Option<&str>, mount: &OsStr) -> Result<(), String> {
    match device {
        Some(node) => detach(program, OsStr::new(node)),
        None => detach(program, mount),
    }
}

/// One DiskImages attachment owned by one test. Dropping it detaches the
/// whole-disk node `hdiutil attach` reported, or the mount point when attach
/// named none, on every exit: a normal return, and a panic from an assertion.
/// Then it removes the scratch directory.
pub struct AttachedImage {
    program: std::ffi::OsString,
    root: std::path::PathBuf,
    // Set once attach has succeeded, so cleanup runs on every later exit.
    mount: Option<std::path::PathBuf>,
    device: Option<String>,
}

impl AttachedImage {
    /// An image that has not attached yet. Dropping it detaches nothing.
    pub fn new(program: impl Into<std::ffi::OsString>, root: std::path::PathBuf) -> Self {
        Self {
            program: program.into(),
            root,
            mount: None,
            device: None,
        }
    }

    /// Record a successful attach, before anything else can fail.
    pub fn attached(&mut self, mount: std::path::PathBuf, device: Option<String>) {
        self.mount = Some(mount);
        self.device = device;
    }
}

impl Drop for AttachedImage {
    fn drop(&mut self) {
        if let Some(mount) = self.mount.take() {
            let device = self.device.take();
            if let Err(error) = detach_attached(
                self.program.as_os_str(),
                device.as_deref(),
                mount.as_os_str(),
            ) {
                let target = device.unwrap_or_else(|| mount.display().to_string());
                let message =
                    format!("detach of {target} failed; the image stays attached: {error}");
                // A second panic during unwinding would abort before the
                // first failure reports, so report it and let that stand.
                if std::thread::panicking() {
                    eprintln!("{message}");
                    return;
                }
                panic!("{message}");
            }
        }
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
