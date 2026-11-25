//! Detach of a DiskImages device node, against stand-in `hdiutil` programs.
//! The stand-ins print the stderr the real tool prints, so the retry rule is
//! exercised without attaching anything to the host.
#![cfg(unix)]
#[path = "support/image_detach.rs"]
mod image_detach;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const BUSY: &str = "hdiutil: couldn't eject \"disk11\" - Resource busy";
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
    let dir = std::env::temp_dir().join(format!("core-detach-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A `hdiutil` stand-in: the first `failing` calls print `stderr` and exit 1,
/// later calls exit 0. Returns the program and its call-count file.
fn stand_in(dir: &Path, stderr: &str, failing: usize) -> (PathBuf, PathBuf) {
    let count = dir.join("count");
    let message = dir.join("stderr");
    fs::write(&message, stderr).unwrap();
    let program = dir.join("hdiutil");
    let script = format!(
        "#!/bin/sh\nn=$(cat '{c}' 2>/dev/null || echo 0)\nn=$((n + 1))\necho \"$n\" > '{c}'\n\
         if [ \"$n\" -le {failing} ]; then cat '{m}' >&2; exit 1; fi\nexit 0\n",
        c = count.display(),
        m = message.display(),
    );
    fs::write(&program, script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    (program, count)
}

fn calls(count: &Path) -> usize {
    fs::read_to_string(count).unwrap().trim().parse().unwrap()
}

#[test]
fn busy_detach_is_retried_until_it_clears() {
    let _serial = serial();
    let dir = scratch("clears");
    let (program, count) = stand_in(&dir, BUSY, 2);
    let result = image_detach::detach(program.as_os_str(), "/dev/disk11".as_ref());
    assert!(
        result.is_ok(),
        "busy detach must retry to success: {result:?}"
    );
    assert_eq!(calls(&count), 3);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn busy_detach_that_never_clears_is_retried_then_reported() {
    let _serial = serial();
    let dir = scratch("never");
    let (program, count) = stand_in(&dir, BUSY, NEVER);
    let result = image_detach::detach(program.as_os_str(), "/dev/disk11".as_ref());
    let error = result.expect_err("a detach that stays busy must fail");
    assert!(
        error.contains("Resource busy"),
        "reported stderr: {error:?}"
    );
    assert!(calls(&count) >= 3, "busy detach retried: {}", calls(&count));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn other_detach_failure_is_reported_without_retry() {
    let _serial = serial();
    let dir = scratch("other");
    let (program, count) = stand_in(&dir, "hdiutil: detach: no such disk", NEVER);
    let result = image_detach::detach(program.as_os_str(), "/dev/disk11".as_ref());
    let error = result.expect_err("a non-busy failure must be reported");
    assert!(error.contains("no such disk"), "reported stderr: {error:?}");
    assert_eq!(calls(&count), 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn attach_that_named_no_node_is_detached_by_mount_point() {
    let _serial = serial();
    let dir = scratch("no-node");
    let (program, count) = stand_in(&dir, "", 0);
    let mount = dir.join("mount");
    image_detach::detach_attached(program.as_os_str(), None, mount.as_os_str())
        .expect("detach by mount point must succeed");
    let made = fs::read_to_string(&count)
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok())
        .unwrap_or(0);
    assert_eq!(
        made, 1,
        "an attached image must be detached even when attach named no node"
    );
    fs::remove_dir_all(dir).unwrap();
}
