//! Progress is checked against captured process streams and real host metadata.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{
    self, Destination, ExportOptions, FileSource, Progress, ProgressOptions,
};
use fs_core::cli::CliError;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

struct Source {
    bytes: u64,
    slow: bool,
}
impl FileSource for Source {
    fn size(&self) -> u64 {
        self.bytes
    }
    fn read_at(&mut self, _: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        if self.slow {
            std::thread::sleep(Duration::from_millis(100));
        }
        let n = if self.slow {
            buf.len().min(1)
        } else {
            buf.len()
        };
        buf[..n].fill(b'x');
        Ok(n)
    }
}
fn options(requested: bool) -> ProgressOptions {
    ProgressOptions {
        progress: requested,
    }
}
fn copy(progress: &mut Progress<'_>, sink: &mut dyn Write, slow: bool) {
    progress.set_totals(Some(23), Some(1));
    transfer::export_file_with_progress(
        &mut Source { bytes: 23, slow },
        Destination::Writer(sink),
        ExportOptions::default(),
        Path::new("a\nfile"),
        progress,
    )
    .unwrap();
}
#[test]
fn flags_are_additive_and_older_matches_default_to_auto() {
    let m = Command::new("export")
        .args(transfer::export_args_with_progress())
        .get_matches_from(["export", "/f", "-", "--progress"]);
    assert!(ProgressOptions::from_matches(&m).progress);
    let m = Command::new("export")
        .args(transfer::export_args())
        .get_matches_from(["export", "/f", "-"]);
    assert!(!ProgressOptions::from_matches(&m).progress);
}
#[test]
fn captured_stdout_contains_only_content_in_every_mode() {
    if let Ok(mode) = std::env::var("TRANSFER_PROGRESS_CHILD") {
        let mut stderr = io::stderr();
        let mut progress = Progress::new(&mut stderr, mode == "terminal", options(mode == "json"));
        if mode == "auto" {
            transfer::export_file(
                &mut Source {
                    bytes: 23,
                    slow: false,
                },
                Destination::Writer(&mut io::stdout()),
            )
            .unwrap();
        } else {
            copy(&mut progress, &mut io::stdout(), false);
        }
        drop(progress);
        std::process::exit(0);
    }
    for mode in ["auto", "silent", "json", "terminal"] {
        let out = Process::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "captured_stdout_contains_only_content_in_every_mode",
                "--quiet",
            ])
            .env("TRANSFER_PROGRESS_CHILD", mode)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        // libtest emits a banner before entering the child; transfer content is its suffix.
        assert!(out.stdout.ends_with(&[b'x'; 23]), "{out:?}");
        assert!(!out.stdout.windows(12).any(|s| s == b"\"file_bytes\""));
        if mode == "silent" || mode == "auto" {
            assert!(out.stderr.is_empty());
        }
        if mode == "json" {
            let lines = std::str::from_utf8(&out.stderr).unwrap();
            assert_eq!(lines.lines().count(), 1);
            for field in [
                "\"file_bytes\": 23",
                "\"file_total\": 23",
                "\"bytes\": 23",
                "\"total_bytes\": 23",
                "\"files\": 1",
                "\"total_files\": 1",
                "\"complete\": true",
            ] {
                assert!(lines.contains(field), "{lines}");
            }
        }
        if mode == "terminal" {
            assert!(out.stderr.starts_with(b"\r\x1b[2K"));
            assert!(out.stderr.ends_with(b"\n"));
            assert_eq!(out.stderr.iter().filter(|b| **b == b'\n').count(), 1);
        }
    }
}
struct Timed(Vec<Instant>);
impl Write for Timed {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        for _ in b.iter().filter(|b| **b == b'\n') {
            self.0.push(Instant::now());
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn slow_source_emits_at_most_one_intermediate_line_per_second() {
    let mut stderr = Timed(Vec::new());
    let start = Instant::now();
    copy(
        &mut Progress::new(&mut stderr, false, options(true)),
        &mut Vec::new(),
        true,
    );
    assert!(stderr.0.len() >= 3, "missing intermediate progress");
    assert!(stderr.0[0].duration_since(start) >= Duration::from_secs(1));
    for pair in stderr.0[..stderr.0.len() - 1].windows(2) {
        assert!(pair[1].duration_since(pair[0]) >= Duration::from_secs(1));
    }
}
#[test]
fn unknown_totals_are_null_and_empty_files_still_finish() {
    let mut stderr = Vec::new();
    let mut progress = Progress::new(&mut stderr, false, options(true));
    progress.begin_file(Path::new("stdin"), None);
    progress.advance(3);
    progress.finish_file(true);
    progress.begin_file(Path::new("empty"), Some(0));
    progress.finish_file(true);
    drop(progress);
    let text = String::from_utf8(stderr).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.contains("\"file_total\": null"));
    assert!(text.contains("\"total_bytes\": null"));
    assert!(text.contains("\"total_files\": null"));
    assert!(text.lines().last().unwrap().contains("\"files\": 2"));
}
#[test]
fn failed_copy_reports_delivered_bytes_without_claiming_completion() {
    struct Broken(bool);
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            if self.0 {
                self.0 = false;
                Ok(1)
            } else {
                Err(io::Error::other("broken"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut stderr = Vec::new();
    let mut progress = Progress::new(&mut stderr, false, options(true));
    assert!(transfer::export_file_with_progress(
        &mut Source {
            bytes: 2,
            slow: false
        },
        Destination::Writer(&mut Broken(true)),
        ExportOptions::default(),
        Path::new("f"),
        &mut progress
    )
    .is_err());
    drop(progress);
    let text = String::from_utf8(stderr).unwrap();
    assert!(text.contains("\"file_bytes\": 1"));
    assert!(text.contains("\"files\": 0"));
    assert!(text.contains("\"complete\": false"));
}
#[test]
fn tree_totals_are_known_before_first_read_and_match_host_sizes() {
    use transfer::{
        ExportRequest, LinkOptions, Output, TimeOptions, TreeEntry, TreeKind, TreeMetadata,
        TreeOptions, TreeSource,
    };
    struct Host(std::path::PathBuf, usize);
    impl TreeSource for Host {
        fn metadata(&mut self, p: &Path) -> Result<TreeMetadata, CliError> {
            let m = std::fs::metadata(self.0.join(p)).unwrap();
            Ok(TreeMetadata {
                kind: if m.is_dir() {
                    TreeKind::Directory
                } else {
                    TreeKind::File
                },
                size: m.len(),
                inode: match p.to_str().unwrap() {
                    "" => 0,
                    "a" => 1,
                    "nested" => 2,
                    _ => 3,
                },
                accessed: None,
                modified: None,
            })
        }
        fn read_dir(&mut self, p: &Path) -> Result<Vec<TreeEntry>, CliError> {
            self.1 += 1;
            let children: &[&str] = if p.as_os_str().is_empty() {
                &["a", "nested"]
            } else {
                &["b"]
            };
            children
                .iter()
                .copied()
                .map(|n| {
                    Ok(TreeEntry {
                        name: n.into(),
                        metadata: self.metadata(&p.join(n))?,
                    })
                })
                .collect()
        }
        fn read_file_at(&mut self, p: &Path, offset: u64, b: &mut [u8]) -> Result<usize, CliError> {
            assert_eq!(
                self.1, 2,
                "all listings must precede content and occur once"
            );
            let bytes = std::fs::read(self.0.join(p)).unwrap();
            let bytes = &bytes[offset as usize..];
            let n = b.len().min(bytes.len());
            b[..n].copy_from_slice(&bytes[..n]);
            Ok(n)
        }
    }
    let root = std::env::temp_dir().join(format!("core-progress-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src/nested")).unwrap();
    std::fs::write(root.join("src/a"), b"abc").unwrap();
    std::fs::write(root.join("src/nested/b"), b"12345").unwrap();
    let mut stderr = Vec::new();
    let mut progress = Progress::new(&mut stderr, false, options(true));
    transfer::export_tree_with_progress(
        &ExportRequest {
            path: "".into(),
            output: Output::File(root.join("dest")),
            replace: false,
        },
        &mut Host(root.join("src"), 0),
        TreeOptions {
            recursive: true,
            ..TreeOptions::default()
        },
        LinkOptions::default(),
        TimeOptions::default(),
        &mut progress,
    )
    .unwrap();
    drop(progress);
    let text = String::from_utf8(stderr).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(
        text.lines()
            .all(|l| l.contains("\"total_bytes\": 8") && l.contains("\"total_files\": 2")),
        "{text}"
    );
    let last = text.lines().last().unwrap();
    assert!(
        last.contains("\"bytes\": 8") && last.contains("\"files\": 2"),
        "{last}"
    );
    assert_eq!(
        std::fs::metadata(root.join("dest/a")).unwrap().len()
            + std::fs::metadata(root.join("dest/nested/b")).unwrap().len(),
        8
    );
    std::fs::remove_dir_all(root).unwrap();
}
