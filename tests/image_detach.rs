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

/// The image each test owns. No tool opens it.
fn own_image(dir: &Path) -> PathBuf {
    dir.join("own.sparseimage")
}

/// An `hdiutil info` text listing each image on its whole-disk node, with its
/// volume mounted at its mount point.
fn listing(images: &[(&Path, &str, &Path)]) -> String {
    let mut text = "framework       : 1\n".to_owned();
    for (image, node, mount) in images {
        text.push_str(&format!(
            "================================================\n\
             image-path      : {}\n\
             {node}\tGUID_partition_scheme\t\n\
             {node}s1\tApple_HFS\t{}\n",
            image.display(),
            mount.display(),
        ));
    }
    text
}

/// A `hdiutil` stand-in: the first `failing` detaches print `stderr` and exit 1,
/// later detaches exit 0. Returns the program and its detach-count file.
///
/// `info` prints the file `listing`, or `listing.N` when that exists and N
/// detaches have run. At first `listing` names the test's own image on
/// `/dev/disk11`, mounted at `mount` in `dir`.
fn stand_in(dir: &Path, stderr: &str, failing: usize) -> (PathBuf, PathBuf) {
    let count = dir.join("count");
    let message = dir.join("stderr");
    fs::write(&message, stderr).unwrap();
    fs::write(
        dir.join("listing"),
        listing(&[(&own_image(dir), "/dev/disk11", &dir.join("mount"))]),
    )
    .unwrap();
    let program = dir.join("hdiutil");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{a}'\nn=$(cat '{c}' 2>/dev/null || echo 0)\n\
         if [ \"$1\" = info ]; then\n\
         f='{l}'\nif [ -e \"$f.$n\" ]; then f=\"$f.$n\"; fi\nexec cat \"$f\"\nfi\n\
         n=$((n + 1))\necho \"$n\" > '{c}'\n\
         if [ \"$n\" -le {failing} ]; then cat '{m}' >&2; exit 1; fi\nexit 0\n",
        c = count.display(),
        a = dir.join("args").display(),
        l = dir.join("listing").display(),
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

/// Detach the test's own image from `/dev/disk11`.
fn detach_node(dir: &Path, program: &Path) -> Result<(), String> {
    image_detach::detach(program.as_os_str(), &own_image(dir), "/dev/disk11".as_ref())
}

#[test]
fn busy_detach_is_retried_until_it_clears() {
    let _serial = serial();
    let dir = scratch("clears");
    let (program, count) = stand_in(&dir, BUSY, 2);
    let result = detach_node(&dir, &program);
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
    let result = detach_node(&dir, &program);
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
    let result = detach_node(&dir, &program);
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
    image_detach::detach_attached(
        program.as_os_str(),
        &own_image(&dir),
        None,
        mount.as_os_str(),
    )
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
        image.attached(
            own_image(&dir),
            mount.clone(),
            Some("/dev/disk11".to_owned()),
        );
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
        image.attached(
            own_image(&dir),
            mount.clone(),
            Some("/dev/disk11".to_owned()),
        );
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
        image.attached(
            own_image(&dir),
            mount.clone(),
            Some("/dev/disk11".to_owned()),
        );
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

/// The calls of one refused detach of `/dev/disk11` that ran no retry.
const ONE_REFUSAL: &str = "info\ndetach /dev/disk11\ninfo\n";

#[test]
fn a_refused_detach_is_retried_while_the_image_stays_on_its_node() {
    let _serial = serial();
    let dir = scratch("stays");
    let (program, _count) = stand_in(&dir, BUSY, 2);
    detach_node(&dir, &program).expect("busy detach must retry to success");
    assert_eq!(
        args_of(&dir),
        "info\ndetach /dev/disk11\ninfo\ndetach -force /dev/disk11\ninfo\ndetach -force /dev/disk11\n"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_node_given_to_another_image_is_not_detached_again() {
    let _serial = serial();
    let dir = scratch("reused");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    let other = dir.join("other.sparseimage");
    fs::write(
        dir.join("listing.1"),
        listing(&[(&other, "/dev/disk11", &dir.join("mount"))]),
    )
    .unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), ONE_REFUSAL);
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_listing_is_read_again_before_every_retry() {
    let _serial = serial();
    let dir = scratch("leaves-later");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    fs::write(dir.join("listing.2"), listing(&[])).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(
        args_of(&dir),
        "info\ndetach /dev/disk11\ninfo\ndetach -force /dev/disk11\ninfo\n"
    );
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_image_that_moved_to_another_node_is_reported_without_a_retry() {
    let _serial = serial();
    let dir = scratch("moved");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    let moved = listing(&[(&own_image(&dir), "/dev/disk12", &dir.join("elsewhere"))]);
    fs::write(dir.join("listing.1"), moved).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), ONE_REFUSAL);
    let error = result.expect_err("a moved image is an error");
    assert!(
        error.contains("is no longer listed on /dev/disk11"),
        "reported: {error:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_listing_that_cannot_be_read_stops_the_retries() {
    let _serial = serial();
    let dir = scratch("unreadable");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    // `cat` of a directory fails, so `info` fails once one detach has run.
    fs::create_dir(dir.join("listing.1")).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), ONE_REFUSAL);
    let error = result.expect_err("an unread listing is an error");
    assert!(error.contains("hdiutil info failed"), "reported: {error:?}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_detach_by_mount_point_is_retried_only_while_the_image_is_mounted_there() {
    let _serial = serial();
    let dir = scratch("mount-retry");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    let mount = dir.join("mount");
    fs::write(dir.join("listing.2"), listing(&[])).unwrap();
    let result = image_detach::detach_attached(
        program.as_os_str(),
        &own_image(&dir),
        None,
        mount.as_os_str(),
    );
    let m = mount.display();
    assert_eq!(
        args_of(&dir),
        format!("info\ndetach {m}\ninfo\ndetach -force {m}\ninfo\n")
    );
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_drop_whose_image_left_its_node_runs_no_further_detach() {
    let _serial = serial();
    let dir = scratch("drop-left");
    let (program, _count) = stand_in(&dir, BUSY, NEVER);
    fs::write(dir.join("listing.1"), listing(&[])).unwrap();
    let image_root = dir.join("image-root");
    fs::create_dir_all(&image_root).unwrap();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut image = AttachedImage::new(program.as_os_str(), image_root.clone());
        image.attached(
            own_image(&dir),
            dir.join("mount"),
            Some("/dev/disk11".to_owned()),
        );
    }));
    assert_eq!(args_of(&dir), ONE_REFUSAL);
    assert!(
        outcome.is_ok(),
        "nothing stays attached, so the drop succeeds"
    );
    assert!(!image_root.exists(), "scratch directory is removed");
    fs::remove_dir_all(dir).unwrap();
}

/// The calls when the listing read before the first detach stops it.
const NO_DETACH: &str = "info\n";

#[test]
fn an_image_no_longer_listed_gets_no_detach_at_all() {
    let _serial = serial();
    let dir = scratch("first-left");
    let (program, _count) = stand_in(&dir, "", 0);
    fs::write(dir.join("listing"), listing(&[])).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), NO_DETACH);
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_node_given_to_another_image_before_the_first_detach_is_left_alone() {
    let _serial = serial();
    let dir = scratch("first-reused");
    let (program, _count) = stand_in(&dir, "", 0);
    let other = dir.join("other.sparseimage");
    fs::write(
        dir.join("listing"),
        listing(&[(&other, "/dev/disk11", &dir.join("mount"))]),
    )
    .unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), NO_DETACH);
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_image_that_moved_before_the_first_detach_is_reported_without_a_detach() {
    let _serial = serial();
    let dir = scratch("first-moved");
    let (program, _count) = stand_in(&dir, "", 0);
    let moved = listing(&[(&own_image(&dir), "/dev/disk12", &dir.join("elsewhere"))]);
    fs::write(dir.join("listing"), moved).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), NO_DETACH);
    let error = result.expect_err("a moved image is an error");
    assert!(
        error.contains("is no longer listed on /dev/disk11"),
        "reported: {error:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_listing_that_cannot_be_read_runs_no_detach() {
    let _serial = serial();
    let dir = scratch("first-unreadable");
    let (program, _count) = stand_in(&dir, "", 0);
    fs::remove_file(dir.join("listing")).unwrap();
    let result = detach_node(&dir, &program);
    assert_eq!(args_of(&dir), NO_DETACH);
    let error = result.expect_err("an unread listing is an error");
    assert!(error.contains("hdiutil info failed"), "reported: {error:?}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_detach_by_mount_point_runs_nothing_when_the_image_is_not_listed() {
    let _serial = serial();
    let dir = scratch("first-mount-left");
    let (program, _count) = stand_in(&dir, "", 0);
    fs::write(dir.join("listing"), listing(&[])).unwrap();
    let result = image_detach::detach_attached(
        program.as_os_str(),
        &own_image(&dir),
        None,
        dir.join("mount").as_os_str(),
    );
    assert_eq!(args_of(&dir), NO_DETACH);
    assert!(
        result.is_ok(),
        "the image is no longer attached: {result:?}"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_drop_whose_image_already_left_its_node_runs_no_detach() {
    let _serial = serial();
    let dir = scratch("first-drop-left");
    let (program, _count) = stand_in(&dir, "", 0);
    fs::write(dir.join("listing"), listing(&[])).unwrap();
    let image_root = dir.join("image-root");
    fs::create_dir_all(&image_root).unwrap();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut image = AttachedImage::new(program.as_os_str(), image_root.clone());
        image.attached(
            own_image(&dir),
            dir.join("mount"),
            Some("/dev/disk11".to_owned()),
        );
    }));
    assert_eq!(args_of(&dir), NO_DETACH);
    assert!(
        outcome.is_ok(),
        "nothing stays attached, so the drop succeeds"
    );
    assert!(!image_root.exists(), "scratch directory is removed");
    fs::remove_dir_all(dir).unwrap();
}
