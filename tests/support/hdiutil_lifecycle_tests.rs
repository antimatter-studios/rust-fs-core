mod lifecycle {
    use super::attach;
    use crate::hdiutil_attachment::{detach_while_owned, hdiutil_info, listing_evidence, Teardown};
    use std::cell::RefCell;
    use std::fs::{self, File};
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Output, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const CHILD_TEST: &str = "macos::lifecycle::attachment_child";
    const FAILING_PROBE_TEST: &str = "macos::lifecycle::a_failing_probe_prints_its_childs_log";
    const FAILING_PROBE_ROOT: &str = "FS_CORE_ATTACH_FAILING_PROBE_ROOT";

    fn own_info(image: &Path) -> Option<String> {
        let output = Command::new("/usr/bin/hdiutil")
            .arg("info")
            .output()
            .unwrap();
        assert!(output.status.success(), "hdiutil info failed");
        let info = String::from_utf8(output.stdout).unwrap();
        info.split("================================================")
            .find(|record| {
                record.lines().any(|line| {
                    line.strip_prefix("image-path")
                        .and_then(|field| field.split_once(": "))
                        .is_some_and(|(_, path)| Path::new(path.trim()) == image)
                })
            })
            .map(str::to_owned)
    }

    fn snapshot(label: &str, image: &Path) -> bool {
        let info = own_info(image);
        println!(
            "hdiutil info [{label}] {}: {}",
            image.display(),
            info.as_deref().unwrap_or("not attached")
        );
        info.is_some()
    }

    fn signal(child: &Child, number: &str) {
        let result = Command::new("/bin/kill")
            .args([number, &child.id().to_string()])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "signal to own child failed: {result:?}"
        );
    }

    fn wait(child: &mut Child) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "own child did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The two handles that give a child's stdout and stderr one log file.
    fn child_log(log: &Path) -> (Stdio, Stdio) {
        // One open file, so both streams advance the same offset.
        let stdout = File::create(log).unwrap();
        let stderr = stdout.try_clone().unwrap();
        (Stdio::from(stdout), Stdio::from(stderr))
    }

    struct Probe {
        child: Child,
        image: PathBuf,
        device: String,
        log: PathBuf,
    }

    impl Probe {
        fn start(root: &Path, label: &str, mode: &str) -> Self {
            let ready = root.join(format!("{label}.ready"));
            let log = root.join(format!("{label}.log"));
            let (stdout, stderr) = child_log(&log);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD_TEST, "--nocapture"])
                .env("TMPDIR", root)
                .env("FS_CORE_ATTACH_CHILD_READY", &ready)
                .env("FS_CORE_ATTACH_CHILD_MODE", mode)
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(90);
            let contents = loop {
                if let Ok(contents) = fs::read_to_string(&ready) {
                    if contents.lines().count() == 2 {
                        break contents;
                    }
                }
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "own attach child exited {status}: {}",
                        fs::read_to_string(&log).unwrap()
                    );
                }
                if Instant::now() >= deadline {
                    signal(&child, "-KILL");
                    wait(&mut child);
                    panic!(
                        "own child did not publish its attachment: {}",
                        fs::read_to_string(&log).unwrap()
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            let mut lines = contents.lines();
            let image = PathBuf::from(lines.next().unwrap());
            let device = lines.next().unwrap().to_owned();
            assert!(
                image.starts_with(root),
                "child attached outside its own root"
            );
            assert!(device.starts_with("/dev/disk"));
            for prefix in std::env::var("FS_CORE_HDIUTIL_FORBIDDEN_DEVICES")
                .unwrap_or_default()
                .split(':')
                .filter(|prefix| !prefix.is_empty())
            {
                assert!(!device.starts_with(prefix), "protected device is forbidden");
            }
            Self {
                child,
                image,
                device,
                log,
            }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            // Before the cleanup below, which can itself fail.
            if std::thread::panicking() {
                match fs::read_to_string(&self.log) {
                    Ok(log) => println!("child log {}:\n{log}", self.log.display()),
                    Err(error) => {
                        println!("child log {} is unreadable: {error}", self.log.display())
                    }
                }
                let info = hdiutil_info().unwrap_or_else(|error| error.to_string());
                println!(
                    "hdiutil info at failure:\n{}",
                    listing_evidence(&self.device, &info)
                );
            }
            if own_info(&self.image).is_some() {
                let result = Command::new("/usr/bin/hdiutil")
                    .args(["detach", "-force", &self.device])
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "own regression image cleanup failed: {result:?}"
                );
            }
            snapshot("regression cleanup", &self.image);
        }
    }

    fn root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "fs_core_lifecycle_{}_{}_{nonce}",
            std::process::id(),
            label
        ));
        fs::create_dir(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }

    #[test]
    fn attachment_child() {
        let Some(ready) = std::env::var_os("FS_CORE_ATTACH_CHILD_READY") else {
            return;
        };
        let mode = std::env::var("FS_CORE_ATTACH_CHILD_MODE").unwrap();
        let fixture = (mode != "unregistered").then(attach);
        let (dir, device) = if let Some(fixture) = &fixture {
            (fixture.dir.clone(), fixture.dev.clone())
        } else {
            let dir = std::env::temp_dir()
                .join("fs_core_attach_device-node_v1")
                .join(format!(
                    "fs_core_attach_{}_unregistered",
                    std::process::id()
                ));
            fs::create_dir_all(&dir).unwrap();
            let image = dir.join("raw.img");
            fs::write(&image, vec![0xAB; super::super::IMAGE_BYTES as usize]).unwrap();
            let output = Command::new("/usr/bin/hdiutil")
                .args([
                    "attach",
                    "-nomount",
                    "-imagekey",
                    "diskimage-class=CRawDiskImage",
                ])
                .arg(&image)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "own unregistered attach failed: {output:?}"
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let device = text
                .split_whitespace()
                .find(|field| field.starts_with("/dev/disk"))
                .unwrap();
            (dir, device.to_owned())
        };
        fs::write(
            ready,
            format!("{}\n{}\n", dir.join("raw.img").display(), device),
        )
        .unwrap();
        snapshot("child before requested exit", &dir.join("raw.img"));
        match mode.as_str() {
            "panic" => panic!("intentional panic after attaching own regression image"),
            "recover" => drop(fixture),
            "wait" | "unregistered" => loop {
                std::thread::sleep(Duration::from_millis(20));
            },
            mode => panic!("unknown child mode {mode}"),
        }
    }

    #[test]
    fn the_childs_log_keeps_what_each_stream_wrote() {
        let root = root("log");
        let log = root.join("child.log");
        let (stdout, stderr) = child_log(&log);
        let status = Command::new("/bin/sh")
            .args(["-c", "echo reason >&2; echo later"])
            .stdout(stdout)
            .stderr(stderr)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(fs::read_to_string(&log).unwrap(), "reason\nlater\n");
        fs::remove_dir_all(root).unwrap();
    }

    const OWN_IMAGE: &str = "/nowhere/raw.img";
    // A node no system has: these tests run no tool.
    const OWN_NODE: &str = "/dev/disk999999999";

    /// An `hdiutil info` text that lists each of `images` on its node.
    fn listing(images: &[(&str, &str)]) -> String {
        let separator = "================================================";
        let mut text = "framework       : 1\n".to_owned();
        for (image, node) in images {
            text.push_str(&format!(
                "{separator}\nimage-path      : {image}\n{node}\t\n"
            ));
        }
        text
    }

    /// Tears down `OWN_IMAGE` on `OWN_NODE` against a fake `hdiutil`. `info`
    /// answers from the number of detaches run so far; each detach gets the next
    /// of `answers` (true succeeds, false is refused as busy), and any detach
    /// after the last of them is refused as busy. Returns the outcome and the
    /// calls made, in order.
    fn teardown_against(
        info: impl Fn(usize) -> io::Result<String>,
        answers: &[bool],
    ) -> (io::Result<Teardown>, Vec<&'static str>) {
        let calls = RefCell::new(Vec::new());
        let detaches = || {
            calls
                .borrow()
                .iter()
                .filter(|call| **call != "info")
                .count()
        };
        let outcome = detach_while_owned(
            Path::new(OWN_IMAGE),
            OWN_NODE,
            || {
                let run = detaches();
                calls.borrow_mut().push("info");
                info(run)
            },
            |force| {
                let succeeds = answers.get(detaches()) == Some(&true);
                calls
                    .borrow_mut()
                    .push(if force { "detach -force" } else { "detach" });
                Ok(Output {
                    status: ExitStatus::from_raw(if succeeds { 0 } else { 1 << 8 }),
                    stdout: Vec::new(),
                    stderr: if succeeds {
                        Vec::new()
                    } else {
                        b"hdiutil: couldn't eject - Resource busy\n".to_vec()
                    },
                })
            },
        );
        (outcome, calls.into_inner())
    }

    #[test]
    fn a_refused_detach_stops_when_the_image_leaves_the_listing() {
        // After the refusal, another image holds the recorded node.
        let (outcome, calls) = teardown_against(
            |detaches| {
                let image = if detaches == 0 {
                    OWN_IMAGE
                } else {
                    "/nowhere/other.img"
                };
                Ok(listing(&[(image, OWN_NODE)]))
            },
            &[],
        );
        assert_eq!(calls, ["info", "detach", "info"]);
        assert!(
            !matches!(outcome, Err(_) | Ok(Teardown::Detached)),
            "an image that left the listing is neither a failure nor a detach"
        );
    }

    #[test]
    fn a_refused_detach_stops_when_the_image_moves_to_another_node() {
        let (outcome, calls) = teardown_against(
            |detaches| {
                let node = if detaches == 0 {
                    OWN_NODE
                } else {
                    "/dev/disk999999998"
                };
                Ok(listing(&[(OWN_IMAGE, node)]))
            },
            &[],
        );
        assert_eq!(calls, ["info", "detach", "info"]);
        let error = outcome
            .err()
            .expect("a moved image is an error")
            .to_string();
        assert!(
            error.contains("no longer names its recorded attach node"),
            "reported: {error}"
        );
    }

    #[test]
    fn a_refused_detach_stops_when_the_listing_cannot_be_read() {
        let (outcome, calls) = teardown_against(
            |detaches| match detaches {
                0 => Ok(listing(&[(OWN_IMAGE, OWN_NODE)])),
                _ => Err(io::Error::other("no listing")),
            },
            &[],
        );
        assert_eq!(calls, ["info", "detach", "info"]);
        let error = outcome.err().expect("an unread listing is an error");
        assert_eq!(error.to_string(), "no listing");
    }

    #[test]
    fn a_refused_detach_is_retried_while_the_image_stays_on_its_node() {
        let (outcome, calls) = teardown_against(
            |detaches| {
                let own: &[(&str, &str)] = &[(OWN_IMAGE, OWN_NODE)];
                Ok(listing(if detaches < 2 { own } else { &[] }))
            },
            &[false, true],
        );
        assert_eq!(calls, ["info", "detach", "info", "detach -force", "info"]);
        assert!(matches!(outcome, Ok(Teardown::Detached)));
    }

    /// Runs itself again as the failing side: a probe that is alive when its
    /// test panics. That probe's child attaches nothing.
    #[test]
    fn a_failing_probe_prints_its_childs_log() {
        if let Some(root) = std::env::var_os(FAILING_PROBE_ROOT) {
            let root = PathBuf::from(root);
            let log = root.join("child.log");
            fs::write(&log, "the child's own reason\n").unwrap();
            let _probe = Probe {
                child: Command::new("/bin/sleep").arg("60").spawn().unwrap(),
                image: root.join("never-attached.img"),
                // A node no system has, so the report has a known answer.
                device: "/dev/disk999999999".to_owned(),
                log,
            };
            panic!("intentional failure while a probe is alive");
        }
        let root = root("failing-probe");
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FAILING_PROBE_TEST])
            .env(FAILING_PROBE_ROOT, &root)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(101), "{output:?}");
        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            printed.contains("intentional failure while a probe is alive"),
            "the failing side did not reach its failure:\n{printed}"
        );
        assert!(
            printed.contains("the child's own reason"),
            "a failing probe did not print its child's log:\n{printed}"
        );
        assert!(
            printed
                .split_once("hdiutil info at failure:")
                .is_some_and(|(_, evidence)| evidence.contains("framework")
                    && evidence.contains("recorded node /dev/disk999999999: absent")),
            "a failing probe did not print what the system listed:\n{printed}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn term_detaches_only_the_childs_own_image() {
        let root = root("term");
        let mut victim = Probe::start(&root, "term", "wait");
        assert!(snapshot("before TERM", &victim.image));
        signal(&victim.child, "-TERM");
        assert!(!wait(&mut victim.child).success());
        assert!(
            !snapshot("after TERM", &victim.image),
            "TERM left its own image attached"
        );
        let log = fs::read_to_string(&victim.log).unwrap();
        let report = format!(
            "owned DiskImages signal 15 cleanup of {}: detached",
            victim.device
        );
        assert!(
            log.contains(&report),
            "the child did not report what its TERM cleanup did"
        );
    }

    /// The image is detached before the signal, so the child's teardown
    /// decides it is not listed. That decision must be reported with what
    /// it was decided from.
    #[test]
    fn term_reports_the_listing_that_did_not_name_its_image() {
        let root = root("unlisted");
        let mut victim = Probe::start(&root, "unlisted", "wait");
        assert!(snapshot("before own detach", &victim.image));
        let detach = Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force", &victim.device])
            .output()
            .unwrap();
        assert!(detach.status.success(), "own detach failed: {detach:?}");
        signal(&victim.child, "-TERM");
        assert!(!wait(&mut victim.child).success());
        let log = fs::read_to_string(&victim.log).unwrap();
        let report = format!(
            "owned DiskImages signal 15 cleanup of {}: not listed by hdiutil info, no detach run",
            victim.device
        );
        assert!(
            log.contains(&report),
            "the child did not report an unlisted image:\n{log}"
        );
        let node = format!("recorded node {}: ", victim.device);
        assert!(
            log.split_once("the listing that did not name it:")
                .is_some_and(
                    |(_, evidence)| evidence.contains(&node) && evidence.contains("framework")
                ),
            "the child did not report the listing it decided from:\n{log}"
        );
    }

    #[test]
    fn panic_detaches_the_childs_own_image() {
        let root = root("panic");
        let mut victim = Probe::start(&root, "panic", "panic");
        assert_eq!(wait(&mut victim.child).code(), Some(101));
        println!("{}", fs::read_to_string(&victim.log).unwrap());
        assert!(
            !snapshot("after panic", &victim.image),
            "panic left its own image attached"
        );
    }

    #[test]
    fn the_next_run_reclaims_a_dead_child_but_not_a_live_child() {
        let root = root("kill");
        let mut victim = Probe::start(&root, "kill", "wait");
        let mut live = Probe::start(&root, "live", "wait");
        let mut unknown = Probe::start(&root, "unregistered", "unregistered");
        signal(&unknown.child, "-KILL");
        assert!(!wait(&mut unknown.child).success());
        assert!(snapshot("before SIGKILL", &victim.image));
        signal(&victim.child, "-KILL");
        assert!(!wait(&mut victim.child).success());
        assert!(snapshot("after SIGKILL, before recovery", &victim.image));
        let mut recovery = Probe::start(&root, "recover", "recover");
        assert!(wait(&mut recovery.child).success());
        assert!(
            !snapshot("after next run", &victim.image),
            "next run left its dead child's image attached"
        );
        assert!(
            snapshot("live sibling after recovery", &live.image),
            "recovery detached a live child's image"
        );
        assert!(
            snapshot("unregistered own fixture after recovery", &unknown.image),
            "recovery detached an unregistered image"
        );
        signal(&live.child, "-TERM");
        assert!(!wait(&mut live.child).success());
        assert!(!snapshot("live sibling after TERM", &live.image));
    }
}
