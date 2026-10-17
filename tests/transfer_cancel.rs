//! Real child processes and the kernel's signals grade cancellation and cleanup.
#![cfg(feature = "cli")]

#[test]
fn ordinary_exports_keep_the_existing_api() {
    struct Empty;
    impl fs_core::cli::transfer::FileSource for Empty {
        fn size(&self) -> u64 {
            0
        }
        fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize, fs_core::cli::CliError> {
            panic!("empty files need no read")
        }
    }
    let mut bytes = Vec::new();
    assert_eq!(
        fs_core::cli::transfer::export_file(
            &mut Empty,
            fs_core::cli::transfer::Destination::Writer(&mut bytes)
        )
        .unwrap()
        .bytes,
        0
    );
}

#[cfg(unix)]
mod unix {
    use fs_core::cli::transfer::{
        self, ExportRequest, FileSource, Output, TreeEntry, TreeKind, TreeMetadata, TreeOptions,
        TreeSource, CHUNK,
    };
    use fs_core::cli::{output, CliError};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    struct Generated(PathBuf);
    impl FileSource for Generated {
        fn size(&self) -> u64 {
            (8 * CHUNK) as u64
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
            if offset == CHUNK as u64 {
                fs::write(self.0.join("ready"), b"ready").unwrap();
                let start = Instant::now();
                while !self.0.join("release").exists() {
                    assert!(
                        start.elapsed() < Duration::from_secs(10),
                        "parent never released the slow source"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            buf.fill(b'x');
            Ok(buf.len())
        }
    }
    impl TreeSource for Generated {
        fn metadata(&mut self, _: &Path) -> Result<TreeMetadata, CliError> {
            Ok(metadata(TreeKind::Directory, 1, 0))
        }
        fn read_dir(&mut self, _: &Path) -> Result<Vec<TreeEntry>, CliError> {
            Ok([
                ("a-complete", 2, 3),
                ("b-slow", 3, (8 * CHUNK) as u64),
                ("c-pending", 4, 3),
            ]
            .into_iter()
            .map(|(name, inode, size)| TreeEntry {
                name: name.into(),
                metadata: metadata(TreeKind::File, inode, size),
            })
            .collect())
        }
        fn read_file_at(
            &mut self,
            path: &Path,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<usize, CliError> {
            if path == Path::new("b-slow") {
                self.read_at(offset, buf)
            } else {
                buf.fill(b'a');
                Ok(buf.len())
            }
        }
    }
    fn metadata(kind: TreeKind, inode: u64, size: u64) -> TreeMetadata {
        TreeMetadata {
            kind,
            size,
            inode,
            accessed: None,
            modified: None,
        }
    }

    fn scenario(name: &str, signal: &str, replace: bool, second: bool, tree: bool) {
        if let Ok(root) = std::env::var("TRANSFER_CANCEL_CHILD") {
            let root = PathBuf::from(root);
            let request = ExportRequest {
                path: if tree { "".into() } else { "b-slow".into() },
                output: Output::File(root.join("dest")),
                replace,
            };
            let mut source = Generated(root);
            let result = if tree {
                transfer::export_tree(
                    &request,
                    &mut source,
                    TreeOptions {
                        recursive: true,
                        ..TreeOptions::default()
                    },
                )
            } else {
                transfer::export(&request, &mut source)
            };
            use std::io::Write;
            let response = output::render("export", output::Format::Json, result);
            std::io::stdout()
                .write_all(response.stdout.as_bytes())
                .unwrap();
            std::io::stderr()
                .write_all(response.stderr.as_bytes())
                .unwrap();
            std::process::exit(i32::from(response.code));
        }
        let root = std::env::temp_dir().join(format!("core-cancel-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        if replace {
            if tree {
                fs::create_dir(root.join("dest")).unwrap();
            }
            fs::write(
                if tree {
                    root.join("dest/b-slow")
                } else {
                    root.join("dest")
                },
                b"old",
            )
            .unwrap();
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("unix::{name}"), "--nocapture"])
            .env("TRANSFER_CANCEL_CHILD", &root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while !root.join("ready").exists() {
            if start.elapsed() > Duration::from_secs(10) || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let out = child.wait_with_output().unwrap();
                panic!("child never reached a chunk boundary: {out:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let send = |sig: &str| {
            assert!(Command::new("kill")
                .args([sig, &child.id().to_string()])
                .status()
                .expect("install the platform kill utility for the signal oracle")
                .success());
        };
        send(signal);
        if second {
            send("-TERM");
        }
        fs::write(root.join("release"), b"go").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(if signal == "-INT" { 130 } else { 143 }),
            "{out:?}"
        );
        let report = String::from_utf8(out.stdout).unwrap();
        assert!(report.contains("\"cancelled\": true"), "{report}");
        assert!(
            report.contains("\"in_flight\"") && report.contains("b-slow"),
            "{report}"
        );
        let dest = if tree {
            root.join("dest/b-slow")
        } else {
            root.join("dest")
        };
        if replace {
            assert_eq!(fs::read(dest).unwrap(), b"old");
        } else {
            assert!(!dest.exists());
        }
        if tree {
            assert_eq!(fs::read(root.join("dest/a-complete")).unwrap(), b"aaa");
            assert!(!root.join("dest/c-pending").exists());
            assert!(
                report.contains("a-complete") && report.contains("c-pending"),
                "{report}"
            );
        }
        let dir = if tree {
            root.join("dest")
        } else {
            root.clone()
        };
        assert!(fs::read_dir(dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sigint_keeps_completed_files_and_stops_the_tree() {
        scenario(
            "sigint_keeps_completed_files_and_stops_the_tree",
            "-INT",
            false,
            false,
            true,
        );
    }
    #[test]
    fn sigterm_keeps_the_old_destination() {
        scenario(
            "sigterm_keeps_the_old_destination",
            "-TERM",
            true,
            false,
            true,
        );
    }
    #[test]
    fn a_second_signal_does_not_bypass_cleanup_or_replace_the_first() {
        scenario(
            "a_second_signal_does_not_bypass_cleanup_or_replace_the_first",
            "-INT",
            true,
            true,
            true,
        );
    }
    #[test]
    fn a_single_file_export_reports_its_interrupted_item() {
        scenario(
            "a_single_file_export_reports_its_interrupted_item",
            "-INT",
            false,
            false,
            false,
        );
    }
}
