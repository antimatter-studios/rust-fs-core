//! Detach one DiskImages device node, as the case oracle's cleanup does.
use std::ffi::OsStr;
use std::process::Command;

/// One `hdiutil detach` of `device`; its stderr is the error on failure.
pub fn detach(program: &OsStr, device: &OsStr) -> Result<(), String> {
    let output = Command::new(program)
        .arg("detach")
        .arg(device)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}
