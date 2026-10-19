//! The operating system's locks, held by another process, are the oracle.
#![cfg(feature = "cli")]

use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fs_core::cli::transfer::{self, Destination, FileSource, ImageLock};
use fs_core::cli::CliError;

#[cfg(unix)]
#[path = "support/fork_holder.rs"]
mod fork_holder;

#[cfg(unix)]
#[test]
fn dropped_guards_release_locks_while_a_forked_child_is_alive() {
    for exclusive in [true, false] {
        let dir = Scratch::new("fork-holder");
        let guard = if exclusive {
            ImageLock::for_import(&dir.image()).unwrap()
        } else {
            ImageLock::for_export(&dir.image(), &mut Vec::new())
                .unwrap()
                .unwrap()
        };
        let child = fork_holder::ForkHolder::new();
        drop(guard);
        // A separate open description must acquire the OS lock immediately,
        // even while the child retains its inherited copy of the old one.
        let contender = File::open(dir.image()).unwrap();
        let result = contender.try_lock();
        drop(child);
        result.unwrap();
    }
}

// Keep loop attach/detach transitions out of unrelated guard assertions.
// Each child process has its own mutex and never constructs Scratch.
static HOST_STATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Scratch {
    root: PathBuf,
    _host_state: std::sync::MutexGuard<'static, ()>,
}
impl Scratch {
    fn new(tag: &str) -> Self {
        let host_state = HOST_STATE.lock().unwrap_or_else(|err| err.into_inner());
        let dir = std::env::temp_dir().join(format!("transfer-lock-{}-{tag}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("image"), b"image bytes").unwrap();
        Self {
            root: dir,
            _host_state: host_state,
        }
    }
    fn image(&self) -> PathBuf {
        self.root.join("image")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

// Re-enter the same test in a separate process. The parent uses std's native
// whole-file locks; only the child uses the transfer guard under test.
fn child(test: &str, image: &Path) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test, "--nocapture"])
        .env("TRANSFER_LOCK_CHILD_IMAGE", image)
        .stdin(Stdio::null());
    cmd
}

#[test]
fn another_process_refuses_import_before_reading_its_input() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        let _guard = match ImageLock::for_import(Path::new(&image)) {
            Ok(guard) => guard,
            Err(err) => {
                eprintln!("{}", err.message);
                std::process::exit(i32::from(err.code));
            }
        };
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        std::process::exit(0);
    }
    let dir = Scratch::new("import");
    let held = File::options()
        .read(true)
        .write(true)
        .open(dir.image())
        .unwrap();
    held.lock().unwrap();
    let input_path = dir.root.join("input");
    fs::write(&input_path, b"producer bytes still unread").unwrap();
    let mut input = File::open(input_path).unwrap();
    let output = child(
        "another_process_refuses_import_before_reading_its_input",
        &dir.image(),
    )
    .stdin(Stdio::from(input.try_clone().unwrap()))
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("lock"));
    assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
    drop(held);
    assert_eq!(fs::read(dir.image()).unwrap(), b"image bytes");
}

#[test]
fn readers_coexist_and_exclude_a_writer_until_the_guards_drop() {
    let dir = Scratch::new("readers");
    let mut warnings = Vec::new();
    let first = ImageLock::for_export(&dir.image(), &mut warnings)
        .unwrap()
        .unwrap();
    let second = ImageLock::for_export(&dir.image(), &mut warnings)
        .unwrap()
        .unwrap();
    assert!(warnings.is_empty());
    let err = ImageLock::for_import(&dir.image()).unwrap_err();
    assert_eq!(err.code, 1);
    drop(first);
    assert!(ImageLock::for_import(&dir.image()).is_err());
    drop(second);
    let writer = ImageLock::for_import(&dir.image()).unwrap();
    assert!(ImageLock::for_import(&dir.image()).is_err());
    drop(writer);
    drop(ImageLock::for_import(&dir.image()).unwrap());
}

struct Source {
    image: PathBuf,
    reads: usize,
}
impl FileSource for Source {
    fn image_path(&self) -> Option<&Path> {
        Some(&self.image)
    }
    fn size(&self) -> u64 {
        3
    }
    fn read_at(&mut self, _: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.reads += 1;
        buf[..3].copy_from_slice(b"abc");
        Ok(3)
    }
}

#[test]
fn export_against_another_writer_continues_and_warns_on_stderr() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        let mut source = Source {
            image: image.into(),
            reads: 0,
        };
        let mut bytes = Vec::new();
        let copied = transfer::export_file(&mut source, Destination::Writer(&mut bytes)).unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(copied.bytes, 3);
        assert_eq!(source.reads, 1);
        return;
    }
    let dir = Scratch::new("export");
    let held = File::options()
        .read(true)
        .write(true)
        .open(dir.image())
        .unwrap();
    held.lock().unwrap();
    let output = child(
        "export_against_another_writer_continues_and_warns_on_stderr",
        &dir.image(),
    )
    .output()
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning"), "{stderr}");
    assert!(stderr.contains("may be changing"), "{stderr}");
    drop(held);
}

#[test]
fn export_holds_its_shared_lock_during_source_reads() {
    struct CheckedSource(Source);
    impl FileSource for CheckedSource {
        fn image_path(&self) -> Option<&Path> {
            self.0.image_path()
        }
        fn size(&self) -> u64 {
            self.0.size()
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
            let contender = File::options()
                .read(true)
                .write(true)
                .open(&self.0.image)
                .unwrap();
            assert!(matches!(
                contender.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            self.0.read_at(offset, buf)
        }
    }
    let dir = Scratch::new("during-read");
    let mut source = CheckedSource(Source {
        image: dir.image(),
        reads: 0,
    });
    transfer::export_file(&mut source, Destination::Writer(&mut Vec::new())).unwrap();
    drop(ImageLock::for_import(&dir.image()).unwrap());
}

#[test]
fn missing_images_fail_closed_before_export_reads() {
    let dir = Scratch::new("missing");
    let image = dir.root.join("missing");
    assert_eq!(ImageLock::for_import(&image).unwrap_err().code, 1);
    let mut source = Source { image, reads: 0 };
    assert_eq!(
        transfer::export_file(&mut source, Destination::Writer(&mut Vec::new()))
            .unwrap_err()
            .code,
        1
    );
    assert_eq!(source.reads, 0);
}

#[test]
fn contention_warning_failures_are_reported() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = Scratch::new("warning");
    let held = File::options()
        .read(true)
        .write(true)
        .open(dir.image())
        .unwrap();
    held.lock().unwrap();
    assert_eq!(
        ImageLock::for_export(&dir.image(), &mut Broken)
            .unwrap_err()
            .code,
        1
    );
    drop(held);
}

#[test]
fn an_import_guard_allows_the_drivers_separate_handle_to_write() {
    let dir = Scratch::new("driver-io");
    let guard = ImageLock::for_import(&dir.image()).unwrap();
    let mut target = File::options()
        .read(true)
        .write(true)
        .open(dir.image())
        .unwrap();
    target.write_all(b"new").unwrap();
    target.sync_all().unwrap();
    assert_eq!(fs::read(dir.image()).unwrap(), b"newge bytes");
    assert!(matches!(
        target.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(guard);
    target.try_lock().unwrap();
    drop(target);
}

#[cfg(target_os = "linux")]
#[path = "support/mounted_loop.rs"]
mod mounted_loop;

#[cfg(target_os = "linux")]
#[test]
fn host_mounted_images_and_devices_refuse_import_with_input_unread() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        match ImageLock::for_import(Path::new(&image)) {
            Err(err) => {
                eprintln!("{}", err.message);
                std::process::exit(i32::from(err.code));
            }
            Ok(_guard) => {
                std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
                std::process::exit(0);
            }
        }
    }
    let dir = Scratch::new("mounted-import");
    let mut volume = mounted_loop::Volume::new(&dir.root, &dir.image(), true);
    let alias = dir.root.join("image alias");
    fs::hard_link(dir.image(), &alias).unwrap();
    for readonly in [false, true] {
        volume.remount(readonly);
        for image in [&dir.image(), &volume.device, &alias] {
            let input_path = dir.root.join("input");
            fs::write(&input_path, b"producer bytes still unread").unwrap();
            let mut input = File::open(input_path).unwrap();
            let output = child(
                "host_mounted_images_and_devices_refuse_import_with_input_unread",
                image,
            )
            .stdin(Stdio::from(input.try_clone().unwrap()))
            .output()
            .unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("mounted"));
            assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
        }
    }
    volume.unmount();
    drop(ImageLock::for_import(&dir.image()).unwrap());
    drop(ImageLock::for_import(&volume.device).unwrap());
}

#[cfg(target_os = "linux")]
#[test]
fn host_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        let mut source = Source {
            image: image.into(),
            reads: 0,
        };
        let mut bytes = Vec::new();
        transfer::export_file(&mut source, Destination::Writer(&mut bytes)).unwrap();
        assert_eq!(bytes, b"abc");
        return;
    }
    let dir = Scratch::new("mounted-export");
    let mut volume = mounted_loop::Volume::new(&dir.root, &dir.image(), false);
    for readonly in [false, true] {
        volume.remount(readonly);
        for image in [&dir.image(), &volume.device] {
            let output = child(
                "host_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not",
                image,
            )
            .output()
            .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(stderr.contains("mounted read-write"), !readonly, "{stderr}");
            assert_eq!(stderr.contains("may be changing"), !readonly, "{stderr}");
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn failed_host_mount_probes_refuse_import_with_input_unread() {
    let dir = Scratch::new("probe-failure");
    let input_path = dir.root.join("input");
    fs::write(&input_path, b"producer bytes still unread").unwrap();
    let mut input = File::open(input_path).unwrap();
    let output = child(
        "host_mounted_images_and_devices_refuse_import_with_input_unread",
        Path::new("/dev/zero"),
    )
    .stdin(Stdio::from(input.try_clone().unwrap()))
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot determine host mount state"));
    assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
}

#[cfg(target_os = "linux")]
#[test]
fn btrfs_members_refuse_import_and_unrelated_devices_stay_usable() {
    let dir = Scratch::new("mounted-btrfs");
    let images = [dir.root.join("member0"), dir.root.join("member1")];
    let mut volume = mounted_loop::Btrfs::new(&dir.root, &images);
    let unrelated_image = dir.root.join("unrelated");
    let unrelated = mounted_loop::Loop::new(&unrelated_image);
    for readonly in [false, true] {
        volume.remount(readonly);
        for image in images.iter().chain(&volume.devices) {
            let input_path = dir.root.join("input");
            fs::write(&input_path, b"producer bytes still unread").unwrap();
            let mut input = File::open(input_path).unwrap();
            let output = child(
                "host_mounted_images_and_devices_refuse_import_with_input_unread",
                image,
            )
            .stdin(Stdio::from(input.try_clone().unwrap()))
            .output()
            .unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("mounted"));
            assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");

            let output = child(
                "host_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not",
                image,
            )
            .output()
            .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(stderr.contains("mounted read-write"), !readonly, "{stderr}");
        }
        // A mounted Btrfs elsewhere on the host says nothing about these.
        drop(ImageLock::for_import(&unrelated.device).unwrap());
        drop(ImageLock::for_import(&unrelated_image).unwrap());
    }
    volume.unmount();
    for image in images.iter().chain(&volume.devices) {
        drop(ImageLock::for_import(image).unwrap());
    }
}

#[cfg(target_os = "macos")]
#[path = "support/mounted_hdiutil.rs"]
mod mounted_hdiutil;

#[cfg(target_os = "macos")]
#[test]
fn macos_attached_images_and_devices_refuse_import_with_input_unread() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        match ImageLock::for_import(Path::new(&image)) {
            Err(err) => {
                eprintln!("{}", err.message);
                std::process::exit(i32::from(err.code));
            }
            Ok(_guard) => {
                std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
                std::process::exit(0);
            }
        }
    }
    if !mounted_hdiutil::host() {
        return;
    }
    let dir = Scratch::new("hdiutil-import");
    let mut volume = mounted_hdiutil::Volume::new(&dir.root, true);
    let alias = dir.root.join("image alias");
    fs::hard_link(&volume.image, &alias).unwrap();
    for readonly in [false, true] {
        volume.remount(readonly);
        for image in [&volume.image, &volume.device, &alias] {
            let input_path = dir.root.join("input");
            fs::write(&input_path, b"producer bytes still unread").unwrap();
            let mut input = File::open(input_path).unwrap();
            let output = child(
                "macos_attached_images_and_devices_refuse_import_with_input_unread",
                image,
            )
            .stdin(Stdio::from(input.try_clone().unwrap()))
            .output()
            .unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("mounted"));
            assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
        }
    }
    // Attached but unmounted is not mounted; after detach the image is free.
    volume.unmount();
    drop(ImageLock::for_import(&volume.device).unwrap());
    volume.detach();
    drop(ImageLock::for_import(&volume.image).unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        let mut source = Source {
            image: image.into(),
            reads: 0,
        };
        let mut bytes = Vec::new();
        transfer::export_file(&mut source, Destination::Writer(&mut bytes)).unwrap();
        assert_eq!(bytes, b"abc");
        return;
    }
    if !mounted_hdiutil::host() {
        return;
    }
    let dir = Scratch::new("hdiutil-export");
    let mut volume = mounted_hdiutil::Volume::new(&dir.root, false);
    for readonly in [false, true] {
        volume.remount(readonly);
        // The mounted raw device is held exclusively by DiskImages, so it
        // cannot be opened for export. The image path remains readable and
        // exercises the mount warning without racing that host lock.
        // The mounted raw device is held exclusively by DiskImages, so it
        // cannot be opened for export. The image path remains readable and
        // exercises the mount warning without racing that host lock.
        let output = child(
            "macos_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not",
            &volume.image,
        )
        .output()
        .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        // DiskImages may hold its own lock on an attached image, which
        // warns separately; only the mount warning tracks the mount mode.
        assert_eq!(stderr.contains("mounted read-write"), !readonly, "{stderr}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_failed_host_mount_probes_refuse_import_with_input_unread() {
    let dir = Scratch::new("hdiutil-probe-failure");
    let input_path = dir.root.join("input");
    fs::write(&input_path, b"producer bytes still unread").unwrap();
    let mut input = File::open(input_path).unwrap();
    let output = child(
        "macos_attached_images_and_devices_refuse_import_with_input_unread",
        Path::new("/dev/zero"),
    )
    .stdin(Stdio::from(input.try_clone().unwrap()))
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot determine host mount state"));
    assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
}

#[cfg(windows)]
#[path = "support/mounted_vdisk.rs"]
mod mounted_vdisk;

#[cfg(windows)]
#[test]
fn windows_attached_images_and_volumes_refuse_import_with_input_unread() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        match ImageLock::for_import(Path::new(&image)) {
            Err(err) => {
                eprintln!("{}", err.message);
                std::process::exit(i32::from(err.code));
            }
            Ok(_guard) => {
                std::io::copy(&mut std::io::stdin(), &mut std::io::sink()).unwrap();
                std::process::exit(0);
            }
        }
    }
    if !mounted_vdisk::host() {
        return;
    }
    let dir = Scratch::new("vdisk-import");
    let mut volume = mounted_vdisk::Volume::new(&dir.root);
    for readonly in [false, true] {
        volume.reattach(readonly);
        for image in [&volume.image, &volume.drive, &volume.volume] {
            let input_path = dir.root.join("input");
            fs::write(&input_path, b"producer bytes still unread").unwrap();
            let mut input = File::open(input_path).unwrap();
            let output = child(
                "windows_attached_images_and_volumes_refuse_import_with_input_unread",
                image,
            )
            .stdin(Stdio::from(input.try_clone().unwrap()))
            .output()
            .unwrap();
            assert_eq!(output.status.code(), Some(1), "{image:?}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("mounted"),
                "{image:?}: {output:?}"
            );
            assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
        }
    }
    // After detach the image is an ordinary file again.
    volume.detach();
    drop(ImageLock::for_import(&volume.image).unwrap());
}

#[cfg(windows)]
#[test]
fn windows_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not() {
    if let Some(image) = std::env::var_os("TRANSFER_LOCK_CHILD_IMAGE") {
        let mut source = Source {
            image: image.into(),
            reads: 0,
        };
        let mut bytes = Vec::new();
        transfer::export_file(&mut source, Destination::Writer(&mut bytes)).unwrap();
        assert_eq!(bytes, b"abc");
        return;
    }
    if !mounted_vdisk::host() {
        return;
    }
    let dir = Scratch::new("vdisk-export");
    let mut volume = mounted_vdisk::Volume::new(&dir.root);
    for readonly in [false, true] {
        volume.reattach(readonly);
        // Windows cannot acquire a file lock on the raw physical-disk path;
        // the import oracle above still verifies mount detection there.
        for image in [&volume.image, &volume.volume] {
            let output = child(
                "windows_read_write_mounts_warn_on_export_stderr_and_read_only_mounts_do_not",
                image,
            )
            .output()
            .unwrap();
            assert!(output.status.success(), "{image:?}: {output:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(
                stderr.contains("mounted read-write"),
                !readonly,
                "{image:?}: {stderr}"
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_failed_host_mount_probes_refuse_import_with_input_unread() {
    let dir = Scratch::new("vdisk-probe-failure");
    let input_path = dir.root.join("input");
    fs::write(&input_path, b"producer bytes still unread").unwrap();
    let mut input = File::open(input_path).unwrap();
    // NUL opens, but it is neither a drive nor a volume, so the probe fails.
    let output = child(
        "windows_attached_images_and_volumes_refuse_import_with_input_unread",
        Path::new(r"\\.\NUL"),
    )
    .stdin(Stdio::from(input.try_clone().unwrap()))
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot check Windows mounts"));
    assert_eq!(input.stream_position().unwrap(), 0, "producer was drained");
}
