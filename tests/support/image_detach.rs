//! Detach one DiskImages device node, as the case oracle's cleanup does.
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
