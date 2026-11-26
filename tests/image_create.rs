//! Creation of a DiskImages fixture, against stand-in `hdiutil` programs. The
//! stand-ins print the stderr the real tool prints, so the retry rule is
//! exercised without creating anything on the host (#444).
#![cfg(unix)]
#[path = "support/image_create.rs"]
mod image_create;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const BUSY: &str = "hdiutil: create failed - Resource busy";
const NEVER: usize = 1000;

/// Held by every test from writing its stand-in to running it. Another test
/// forking while this one still has the script open for writing hands the
/// child that descriptor, and exec then fails with "Text file busy" (ETXTBSY).
static SPAWN: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SPAWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("core-create-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A `hdiutil` stand-in. Each call logs whether its image already existed, then
/// writes it. The first `failing` calls print `stderr` and exit 1, leaving that
/// partial image behind.
fn stand_in(dir: &Path, stderr: &str, failing: usize) -> PathBuf {
    let message = dir.join("stderr");
    fs::write(&message, stderr).unwrap();
    let program = dir.join("hdiutil");
    let script = format!(
        "#!/bin/sh\nfor image; do :; done\n\
         if [ -e \"$image\" ]; then echo present >> '{log}'; else echo absent >> '{log}'; fi\n\
         echo partial > \"$image\"\n\
         if [ \"$(wc -l < '{log}')\" -le {failing} ]; then cat '{m}' >&2; exit 1; fi\nexit 0\n",
        log = dir.join("calls").display(),
        m = message.display(),
    );
    fs::write(&program, script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    program
}

/// For each call, whether the image was there when it started.
fn calls(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn create(program: &Path, image: &Path) -> Result<(), String> {
    image_create::create(program.as_os_str(), &[OsStr::new("create")], image)
}

#[test]
fn a_busy_create_is_retried_over_a_removed_partial_image() {
    let _serial = serial();
    let dir = scratch("clears");
    let program = stand_in(&dir, BUSY, 2);
    let image = dir.join("image");
    let result = create(&program, &image);
    assert!(
        result.is_ok(),
        "busy create must retry to success: {result:?}"
    );
    assert_eq!(calls(&dir), ["absent", "absent", "absent"]);
    assert!(image.exists(), "the created image is kept");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_create_that_stays_busy_is_reported_as_a_host_prerequisite() {
    let _serial = serial();
    let dir = scratch("never");
    let program = stand_in(&dir, BUSY, NEVER);
    let error =
        create(&program, &dir.join("image")).expect_err("a create that stays busy must fail");
    assert!(error.contains("Resource busy"), "reported: {error:?}");
    assert!(
        error.contains("another process on this host"),
        "the failure names what to look for: {error:?}"
    );
    assert_eq!(calls(&dir).len(), image_create::ATTEMPTS as usize);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn any_other_create_failure_is_reported_without_retry() {
    let _serial = serial();
    let dir = scratch("other");
    let program = stand_in(
        &dir,
        "hdiutil: create failed - No space left on device",
        NEVER,
    );
    let error = create(&program, &dir.join("image")).expect_err("a refusal must be reported");
    assert!(error.contains("No space left"), "reported: {error:?}");
    assert_eq!(calls(&dir).len(), 1);
    fs::remove_dir_all(dir).unwrap();
}
