//! Creating a DiskImages fixture. Each test crate includes this file.
#![allow(dead_code)]
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Create attempts before a busy answer is reported.
pub const ATTEMPTS: u32 = 6;

/// Run `program args... image`, which creates `image`. On failure returns what
/// the tool answered.
pub fn create(program: &OsStr, args: &[&OsStr], image: &Path) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .arg(image)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).into_owned())
}
