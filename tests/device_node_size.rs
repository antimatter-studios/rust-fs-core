//! A device node has to report its own length.
//!
//! `FileDevice`'s first doc line names "raw `/dev/diskN` reads" as a
//! supported source, and the size used to come from `metadata().len()`,
//! which is 0 for every device node. The device still served real bytes,
//! so nothing errored -- `size_bytes()` simply said 0 and every consumer
//! believed it.
//!
//! The witness tests here open a **real device node** over a backing
//! store of known length. They construct the fixture rather than looking
//! for one, and they FAIL rather than skip if it cannot be built: a test
//! that quietly finds no device is indistinguishable from a test that
//! found a working one.

use fs_core::{BlockRead, FileDevice};

/// 4 MiB, an exact multiple of any plausible device block size, so the
/// expected answer is the file length itself and not a rounding of it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const IMAGE_BYTES: u64 = 4 * 1024 * 1024;

// ---------------------------------------------------------------
// Regular files are unaffected. These run on every platform.
// ---------------------------------------------------------------

/// A regular file still takes its size from its metadata, including the
/// legitimately-empty case. The fix must not turn "this file is empty"
/// into "this size is unmeasurable".
#[test]
fn a_regular_file_still_reports_its_metadata_length() {
    let dir = std::env::temp_dir().join(format!("fs_core_dns_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the fixture directory");

    let full = dir.join("full.img");
    std::fs::write(&full, vec![7u8; 3000]).expect("write the non-empty image");
    let dev = FileDevice::open(&full).expect("open a regular file");
    assert_eq!(
        dev.size_bytes(),
        3000,
        "a regular file's size is its length"
    );

    let empty = dir.join("empty.img");
    std::fs::write(&empty, []).expect("write the empty image");
    let dev = FileDevice::open(&empty).expect("an empty regular file still opens");
    assert_eq!(
        dev.size_bytes(),
        0,
        "0 is the right answer for an empty regular file, and must stay an answer"
    );

    std::fs::remove_file(&full).ok();
    std::fs::remove_file(&empty).ok();
    std::fs::remove_dir(&dir).ok();
}

/// A device node whose size cannot be measured is an ERROR, not a 0.
///
/// `/dev/zero` is a character device on both Unix platforms this crate
/// ships to, and it is not a disk: the size ioctl fails on it with
/// `ENOTTY`. It used to open happily and report `size_bytes() == 0`,
/// which is indistinguishable from an empty file, from a null FFI
/// handle, and from a real disk that could not be measured.
///
/// This is the one arm of the device behaviour that needs no privileges
/// and no fixture, so it is the arm that actually runs on every Unix CI
/// runner.
#[cfg(unix)]
#[test]
fn a_device_whose_size_cannot_be_measured_refuses_to_open() {
    let err = match FileDevice::open("/dev/zero") {
        Err(e) => e,
        Ok(dev) => panic!(
            "/dev/zero opened and reported {} bytes; a size nobody measured must not \
             be handed out as though it were one",
            dev.size_bytes()
        ),
    };
    // Named, so the caller can tell "unmeasurable" from "empty".
    assert!(
        matches!(err, fs_core::Error::Io(_)),
        "expected an Io error naming the failed measurement, got {err:?}"
    );
}

// ---------------------------------------------------------------
// macOS: hdiutil attaches a raw image unprivileged, giving a block node
// (/dev/diskN) and a character node (/dev/rdiskN) over the same bytes.
// ---------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Detaches on drop, so a failing assertion does not leak a device.
    struct Attached {
        dir: PathBuf,
        dev: String,
    }

    impl Drop for Attached {
        fn drop(&mut self) {
            Command::new("hdiutil")
                .args(["detach", &self.dev, "-force"])
                .output()
                .ok();
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn attach() -> Attached {
        attach_with_block_size(None)
    }

    fn attach_with_block_size(block_size: Option<u32>) -> Attached {
        let dir = std::env::temp_dir().join(format!(
            "fs_core_attach_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let img = dir.join("raw.img");
        std::fs::write(&img, vec![0xABu8; IMAGE_BYTES as usize]).expect("write the raw image");
        assert_eq!(
            std::fs::metadata(&img).expect("stat the image").len(),
            IMAGE_BYTES,
            "the backing file is not the length this test assumes"
        );

        let mut cmd = Command::new("hdiutil");
        cmd.args(["attach", "-nomount"]);
        let bs;
        if let Some(n) = block_size {
            bs = n.to_string();
            cmd.args(["-blocksize", &bs]);
        }
        cmd.args([
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            img.to_str().expect("image path is utf-8"),
        ]);
        let out = cmd.output().expect("run hdiutil");

        // NOT a skip. If the fixture cannot be built, this test has
        // measured nothing, and reporting that as success is the exact
        // defect it exists to catch.
        assert!(
            out.status.success(),
            "hdiutil attach failed, so no device node was tested: status={:?} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let dev = stdout
            .split_whitespace()
            .find(|t| t.starts_with("/dev/disk"))
            .unwrap_or_else(|| panic!("hdiutil named no /dev/disk device; stdout was {stdout:?}"))
            .to_string();

        Attached { dir, dev }
    }

    /// THE WITNESS. A block device node reports the length of what it is
    /// backed by, not 0.
    #[test]
    fn a_block_device_node_reports_its_real_length() {
        let a = attach();
        let dev = FileDevice::open(&a.dev).expect("open the block device node");
        assert_eq!(
            dev.size_bytes(),
            IMAGE_BYTES,
            "{} reported {} bytes; st_size for a device node is 0, so this is \
             the metadata length leaking through rather than a measurement",
            a.dev,
            dev.size_bytes()
        );
    }

    /// The character node is a separate `st_mode` case and a separate
    /// branch of the size probe, so it gets its own assertion.
    #[test]
    fn a_character_device_node_reports_its_real_length() {
        let a = attach();
        let raw = a.dev.replace("/dev/disk", "/dev/rdisk");
        let dev = FileDevice::open(&raw).expect("open the character device node");
        assert_eq!(
            dev.size_bytes(),
            IMAGE_BYTES,
            "{raw} reported {} bytes rather than its real length",
            dev.size_bytes()
        );
    }

    /// The block size is READ from the device, not assumed to be 512.
    ///
    /// macOS answers the size in two parts and the product is the only
    /// answer; a probe that hardcoded the common 512 would agree with
    /// every 512-byte device and be wrong by a factor of 8 here.
    /// `hdiutil -blocksize 4096` gives a device reporting 1024 blocks of
    /// 4096, so an assumed 512 yields 524288 rather than 4194304.
    #[test]
    fn the_block_size_comes_from_the_device_rather_than_a_constant() {
        let a = attach_with_block_size(Some(4096));
        let dev = FileDevice::open(&a.dev).expect("open the 4096-byte-block device node");
        assert_eq!(
            dev.size_bytes(),
            IMAGE_BYTES,
            "{} reported {} bytes; 524288 would mean the block count was \
             multiplied by an assumed 512 rather than the device's own 4096",
            a.dev,
            dev.size_bytes()
        );
    }

    /// The user-visible symptom from the issue, end to end: streaming a
    /// whole raw disk used to return an empty vector and `Ok`, because
    /// `BlockReadStreamer::read` opens with `if self.pos >= size`.
    ///
    /// This asserts on the BYTES, not just the count, so a device that
    /// reported the right length while reading rubbish still fails.
    #[test]
    fn streaming_a_raw_device_yields_its_bytes_rather_than_nothing() {
        use std::io::Read;
        let a = attach();
        let dev = FileDevice::open(&a.dev).expect("open the block device node");
        let mut streamer = fs_core::BlockReadStreamer::new(dev);
        let mut got = Vec::new();
        streamer.read_to_end(&mut got).expect("stream the device");
        assert_eq!(
            got.len() as u64,
            IMAGE_BYTES,
            "read_to_end over a raw device returned {} bytes",
            got.len()
        );
        assert!(
            got.iter().all(|b| *b == 0xAB),
            "the bytes streamed off the device are not the bytes written to it"
        );
    }
}

// ---------------------------------------------------------------
// Linux: losetup needs root, so this runs where the suite has it (a
// container, or a root CI job) and says plainly when it does not. The
// macOS arm above is the one that runs unprivileged in CI.
// ---------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn is_root() -> bool {
        // SAFETY: `geteuid` takes nothing and cannot fail.
        unsafe { geteuid() == 0 }
    }

    unsafe extern "C" {
        fn geteuid() -> u32;
    }

    struct Loop {
        dir: PathBuf,
        dev: String,
    }

    impl Drop for Loop {
        fn drop(&mut self) {
            Command::new("losetup")
                .args(["-d", &self.dev])
                .output()
                .ok();
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn attach() -> Loop {
        let dir = std::env::temp_dir().join(format!("fs_core_loop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let img = dir.join("raw.img");
        std::fs::write(&img, vec![0xABu8; IMAGE_BYTES as usize]).expect("write the raw image");

        let out = Command::new("losetup")
            .args(["--find", "--show", img.to_str().expect("path is utf-8")])
            .output()
            .expect("run losetup");
        assert!(
            out.status.success(),
            "losetup failed, so no device node was tested: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(
            dev.starts_with("/dev/loop"),
            "losetup named {dev:?} rather than a loop device"
        );
        Loop { dir, dev }
    }

    /// THE WITNESS on Linux. Same assertion, different mechanism behind
    /// it: `BLKGETSIZE64` rather than the pair of macOS calls.
    #[test]
    fn a_loop_device_reports_its_real_length() {
        if !is_root() {
            // THIS DOES NOT eprintln! AND RETURN, WHICH IS WHAT IT USED
            // TO DO. libtest captures stdout AND stderr of a PASSING
            // test and discards them, so the "loud" line was written
            // and thrown away: measured, `cargo test --locked
            // --all-targets` (what ci.yml runs) showed the marker 0
            // times, and only `-- --nocapture` showed it once. A skip
            // nobody can see is the silent pass this file's own header
            // refuses.
            //
            // So it FAILS instead, unless the environment explicitly
            // accepts the gap. That puts the acknowledgement in
            // ci.yml, where a person reads it in review, rather than
            // in scrollback nothing prints.
            assert!(
                std::env::var_os("AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP").is_some(),
                "the Linux device-size probe's SUCCESS path was NOT exercised: \
                 losetup needs root and euid is not 0. Run the suite as root \
                 (`cargo test --test device_node_size`) to cover it, or set \
                 AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP=1 to accept the gap. \
                 Note the FAILURE path of the same probe IS covered here \
                 unprivileged, by a_device_whose_size_cannot_be_measured_refuses_to_open."
            );
            return;
        }
        let l = attach();
        let dev = FileDevice::open(&l.dev).expect("open the loop device");
        assert_eq!(
            dev.size_bytes(),
            IMAGE_BYTES,
            "{} reported {} bytes rather than its real length",
            l.dev,
            dev.size_bytes()
        );
    }
}
