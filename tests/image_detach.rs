//! Detach of a DiskImages device node, against stand-in `hdiutil` programs.
//! The stand-ins print the stderr the real tool prints, so the retry rule is
//! exercised without attaching anything to the host.
#![cfg(unix)]
#[path = "support/image_detach.rs"]
mod image_detach;

use image_detach::AttachedImage;
use std::panic::{self, AssertUnwindSafe};

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
         echo \"$*\" >> '{a}'\n\
         if [ \"$n\" -le {failing} ]; then cat '{m}' >&2; exit 1; fi\nexit 0\n",
        c = count.display(),
        a = dir.join("args").display(),
        m = message.display(),
    );
    fs::write(&program, script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    (program, count)
}

/// The arguments of each `hdiutil` call, one line per call.
fn args_of(dir: &Path) -> String {
    fs::read_to_string(dir.join("args")).unwrap_or_default()
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

#[test]
fn an_assertion_failure_after_attach_still_detaches_the_node() {
    let _serial = serial();
    let dir = scratch("unwind-clears");
    let (program, count) = stand_in(&dir, "", 0);
    let mount = dir.join("mount");
    let image_root = dir.join("image-root");
    fs::create_dir_all(&image_root).unwrap();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut image = AttachedImage::new(program.as_os_str(), image_root.clone());
        image.attached(mount.clone(), Some("/dev/disk11".to_owned()));
        panic!("assertion failed inside the case oracle");
    }));
    assert!(outcome.is_err(), "the assertion must still unwind");
    assert_eq!(calls(&count), 1, "the node must be detached on unwind");
    assert!(
        args_of(&dir).contains("detach /dev/disk11"),
        "only the reported node is detached: {:?}",
        args_of(&dir)
    );
    assert!(
        !image_root.exists(),
        "image scratch directory is removed on unwind"
    );
}

#[test]
fn a_busy_node_during_unwind_is_retried_and_the_assertion_still_reports() {
    let _serial = serial();
    let dir = scratch("unwind-busy");
    let (program, count) = stand_in(&dir, BUSY, NEVER);
    let mount = dir.join("mount");
    let image_root = dir.join("image-root");
    fs::create_dir_all(&image_root).unwrap();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut image = AttachedImage::new(program.as_os_str(), image_root.clone());
        image.attached(mount.clone(), Some("/dev/disk11".to_owned()));
        panic!("assertion failed inside the case oracle");
    }));
    // Reaching this line at all shows the failed detach did not abort the
    // process with a second panic; the original assertion is what reports.
    let payload = outcome.expect_err("the assertion must still unwind");
    let text = payload.downcast_ref::<&str>().copied().unwrap_or_default();
    assert_eq!(text, "assertion failed inside the case oracle");
    assert!(calls(&count) >= 3, "busy detach retried: {}", calls(&count));
}

#[test]
fn a_detach_that_fails_on_a_normal_exit_panics_naming_the_node() {
    let _serial = serial();
    let dir = scratch("normal-busy");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    let mount = dir.join("mount");
    let image_root = dir.join("image-root");
    fs::create_dir_all(&image_root).unwrap();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut image = AttachedImage::new(program.as_os_str(), image_root.clone());
        image.attached(mount.clone(), Some("/dev/disk11".to_owned()));
    }));
    let payload = outcome.expect_err("a failed detach on a normal exit must panic");
    let text = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        text.contains("detach of /dev/disk11 failed; the image stays attached"),
        "reported: {text:?}"
    );
    assert!(text.contains("Resource busy"), "reported: {text:?}");
}

#[test]
fn an_image_that_never_attached_detaches_nothing() {
    let _serial = serial();
    let dir = scratch("never-attached");
    let (program, count) = stand_in(&dir, "", 0);
    drop(AttachedImage::new(program.as_os_str(), dir.clone()));
    assert!(
        !count.exists(),
        "hdiutil must not run for an image that never attached"
    );
    assert!(!dir.exists(), "scratch directory is removed");
}
