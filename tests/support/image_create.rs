//! Creating a DiskImages fixture. Each test crate includes this file.
#![allow(dead_code)]
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Create attempts before a busy answer is reported.
pub const ATTEMPTS: u32 = 6;

/// Run `program args... image`, which creates `image`, retrying while it answers
/// "Resource busy" (#444). A partial image is removed before another attempt,
/// and each retry is printed, so a busy host is visible in a passing run too.
/// Any other failure returns the tool's stderr at once.
pub fn create(program: &OsStr, args: &[&OsStr], image: &Path) -> Result<(), String> {
    for attempt in 1..=ATTEMPTS {
        let output = Command::new(program)
            .args(args)
            .arg(image)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !stderr.to_ascii_lowercase().contains("resource busy") {
            return Err(stderr);
        }
        if attempt == ATTEMPTS {
            return Err(format!(
                "{ATTEMPTS} attempts answered \"Resource busy\": another process on this \
                 host is holding DiskImages (a disk scanner does this). Stop it and run \
                 again. Last answer: {stderr}"
            ));
        }
        eprintln!("image create attempt {attempt} of {ATTEMPTS} was busy, retrying: {stderr}");
        match std::fs::remove_file(image) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove the partial image: {error}")),
        }
        std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
    }
    unreachable!("the loop returns on its last attempt")
}
