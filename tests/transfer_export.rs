//! `fs_core::cli::transfer`: one file copied out of an image, the way a
//! driver's `export` verb drives it — a [`FileSource`] over the driver's
//! positioned read, and a destination that is a writer (stdout) or a local
//! file.
//!
//! The digest is checked against the platform's own SHA-256 tool, not
//! against this crate: a digest computed by the code under test and checked
//! by the same code would agree with itself whatever it computed.
//!
//! Built only with the `cli` feature; `chore test:debug` and CI pass it.

#![cfg(feature = "cli")]

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use fs_core::cli::clap::Command as Cmd;
use fs_core::cli::transfer::{self, Destination, ExportRequest, FileSource, Output, CHUNK};
use fs_core::cli::{output, CliError, Json};

// ---------------------------------------------------------------------------
// Doubles
// ---------------------------------------------------------------------------

/// A file held in memory and read at a position, as a driver reads one. It
/// can claim to be longer than it is (a short read), fail every read that
/// reaches a given offset (unreadable blocks), or answer a read with more
/// bytes than were asked for (a defective driver).
struct Source {
    data: Vec<u8>,
    claimed: Option<u64>,
    fail_from: Option<u64>,
    overlong: bool,
    reads: usize,
    largest_read: usize,
}

impl Source {
    fn new(data: Vec<u8>) -> Source {
        Source {
            data,
            claimed: None,
            fail_from: None,
            overlong: false,
            reads: 0,
            largest_read: 0,
        }
    }
}

impl FileSource for Source {
    fn size(&self) -> u64 {
        self.claimed.unwrap_or(self.data.len() as u64)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        self.reads += 1;
        self.largest_read = self.largest_read.max(buf.len());
        let end = offset + buf.len() as u64;
        if self.fail_from.is_some_and(|from| end > from) {
            return Err(CliError::failed(format!(
                "/f: blocks unreadable at byte {offset}"
            )));
        }
        if self.overlong {
            return Ok(buf.len() + 1);
        }
        let start = (offset as usize).min(self.data.len());
        let n = buf.len().min(self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }
}

/// A writer that takes `accept` bytes and then fails, as a closed pipe does.
struct ClosesAfter {
    accept: usize,
    taken: usize,
}

impl Write for ClosesAfter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.taken >= self.accept {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "reader went away",
            ));
        }
        let n = buf.len().min(self.accept - self.taken);
        self.taken += n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `len` bytes that differ from chunk to chunk, so a chunk copied twice or
/// skipped changes the digest.
fn pattern(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rust-fs-core-transfer-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every name in `dir`, sorted: what a copy left behind, staging files
/// included.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn file(path: &Path, replace: bool) -> Destination<'_> {
    Destination::File { path, replace }
}

// ---------------------------------------------------------------------------
// The independent oracle: the platform's SHA-256 tool
// ---------------------------------------------------------------------------

/// The SHA-256 of each file, in lowercase hex, as the platform's own tool
/// computes it. A missing tool fails the test, naming what would provide
/// it: a skipped digest check reads exactly like a passing one.
fn platform_sha256(files: &[PathBuf]) -> Vec<String> {
    let out = if cfg!(windows) {
        let list = files
            .iter()
            .map(|f| format!("'{}'", f.display()))
            .collect::<Vec<_>>()
            .join(",");
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "Get-FileHash -Algorithm SHA256 -LiteralPath {list} | ForEach-Object {{ $_.Hash }}"
            ))
            .output()
            .expect("powershell (Get-FileHash) is the SHA-256 oracle on Windows")
    } else {
        match Command::new("sha256sum").args(files).output() {
            Ok(out) => out,
            Err(_) => Command::new("shasum")
                .args(["-a", "256"])
                .args(files)
                .output()
                .expect("no SHA-256 tool: install coreutils (sha256sum) or perl (shasum)"),
        }
    };
    assert!(out.status.success(), "the SHA-256 tool failed: {out:?}");
    let digests: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split_whitespace().next().unwrap().to_ascii_lowercase())
        .collect();
    assert_eq!(digests.len(), files.len(), "one digest per file");
    digests
}

// ---------------------------------------------------------------------------
// The bytes and their digest
// ---------------------------------------------------------------------------

#[test]
fn digest_matches_the_platform_sha256_tool_across_chunk_boundaries() {
    let dir = scratch("digest");
    let sizes = [0, 1, 4096, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 1];
    let mut published = Vec::new();
    let mut ours = Vec::new();
    for size in sizes {
        let data = pattern(size);
        let dest = dir.join(format!("f{size}"));
        let copied = transfer::export_file(&mut Source::new(data.clone()), file(&dest, false))
            .unwrap_or_else(|e| panic!("export of {size} bytes: {}", e.message));
        assert_eq!(copied.bytes, size as u64);
        assert!(!copied.replaced);
        assert_eq!(std::fs::read(&dest).unwrap(), data, "{size} bytes");
        published.push(dest);
        ours.push(copied.sha256_hex());
    }
    assert_eq!(ours, platform_sha256(&published));
}

#[test]
fn digest_of_abc_is_the_published_test_vector() {
    // FIPS 180-2, appendix B.1: the one-block message "abc".
    let mut out = Vec::new();
    let copied = transfer::export_file(
        &mut Source::new(b"abc".to_vec()),
        Destination::Writer(&mut out),
    )
    .unwrap();
    assert_eq!(
        copied.sha256_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_writer_receives_every_byte_in_order() {
    let data = pattern(2 * CHUNK + 17);
    let mut source = Source::new(data.clone());
    let mut out = Vec::new();
    let copied = transfer::export_file(&mut source, Destination::Writer(&mut out)).unwrap();
    assert_eq!(out, data);
    assert_eq!(copied.bytes, data.len() as u64);
    assert!(
        source.largest_read <= CHUNK,
        "a read of {} bytes asked for more than one chunk",
        source.largest_read
    );
}

// ---------------------------------------------------------------------------
// Publication: complete or not at all, and never over a file unasked
// ---------------------------------------------------------------------------

#[test]
fn an_existing_destination_is_refused_without_replace_before_any_read() {
    let dir = scratch("no-replace");
    let dest = dir.join("out");
    std::fs::write(&dest, b"old").unwrap();
    let mut source = Source::new(pattern(100));
    let err = transfer::export_file(&mut source, file(&dest, false)).unwrap_err();
    assert_eq!(err.code, output::EXIT_FAILED);
    assert!(err.message.contains("exists"), "{}", err.message);
    assert!(err.message.contains("--replace"), "{}", err.message);
    assert_eq!(
        source.reads, 0,
        "the image was read for a copy that could not publish"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert_eq!(names(&dir), ["out"]);
}

#[test]
fn replace_swaps_in_the_new_file_and_says_so() {
    let dir = scratch("replace");
    let dest = dir.join("out");
    std::fs::write(&dest, b"old").unwrap();
    let data = pattern(CHUNK + 3);
    let copied = transfer::export_file(&mut Source::new(data.clone()), file(&dest, true)).unwrap();
    assert!(copied.replaced);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert_eq!(names(&dir), ["out"]);
}

#[test]
fn a_source_failing_mid_copy_keeps_the_old_file_and_leaves_no_staging_name() {
    let dir = scratch("source-fails");
    let dest = dir.join("out");
    std::fs::write(&dest, b"old").unwrap();
    let mut source = Source::new(pattern(3 * CHUNK));
    source.fail_from = Some(2 * CHUNK as u64 + 5);
    let err = transfer::export_file(&mut source, file(&dest, true)).unwrap_err();
    assert!(err.message.contains("unreadable"), "{}", err.message);
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert_eq!(names(&dir), ["out"]);

    // And with nothing there before, nothing is there after.
    let fresh = dir.join("fresh");
    let mut source = Source::new(pattern(3 * CHUNK));
    source.fail_from = Some(CHUNK as u64);
    transfer::export_file(&mut source, file(&fresh, false)).unwrap_err();
    assert_eq!(names(&dir), ["out"]);
}

#[test]
fn a_short_read_fails_and_publishes_nothing() {
    let dir = scratch("short");
    let dest = dir.join("out");
    let mut source = Source::new(pattern(CHUNK + 10));
    source.claimed = Some(2 * CHUNK as u64);
    let err = transfer::export_file(&mut source, file(&dest, false)).unwrap_err();
    assert!(err.message.contains("short read"), "{}", err.message);
    assert!(
        err.message
            .contains(&format!("{} of {}", CHUNK + 10, 2 * CHUNK)),
        "{}",
        err.message
    );
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_source_answering_more_than_was_asked_is_refused() {
    let mut source = Source::new(pattern(10));
    source.overlong = true;
    let err = transfer::export_file(&mut source, Destination::Writer(&mut Vec::new())).unwrap_err();
    assert!(err.message.contains("asked for"), "{}", err.message);
}

#[test]
fn a_failing_writer_reports_how_much_was_delivered_of_the_total() {
    let total = 3 * CHUNK;
    let mut sink = ClosesAfter {
        accept: CHUNK,
        taken: 0,
    };
    let err = transfer::export_file(
        &mut Source::new(pattern(total)),
        Destination::Writer(&mut sink),
    )
    .unwrap_err();
    assert_eq!(err.code, output::EXIT_FAILED);
    assert!(
        err.message.contains(&format!("{CHUNK} of {total} bytes")),
        "{}",
        err.message
    );
}

#[test]
fn a_missing_destination_directory_is_not_created() {
    let dir = scratch("no-parent");
    let dest = dir.join("missing").join("out");
    let err = transfer::export_file(&mut Source::new(pattern(10)), file(&dest, false)).unwrap_err();
    assert!(err.message.contains("missing"), "{}", err.message);
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_directory_destination_is_refused_even_with_replace() {
    let dir = scratch("dir-dest");
    let dest = dir.join("sub");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("keep"), b"kept").unwrap();
    let err = transfer::export_file(&mut Source::new(pattern(10)), file(&dest, true)).unwrap_err();
    assert!(err.message.contains("directory"), "{}", err.message);
    assert_eq!(std::fs::read(dest.join("keep")).unwrap(), b"kept");
    assert_eq!(names(&dir), ["sub"]);
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_the_destination_is_an_existing_name() {
    let dir = scratch("symlink-dest");
    let dest = dir.join("out");
    std::os::unix::fs::symlink(dir.join("nowhere"), &dest).unwrap();
    let err = transfer::export_file(&mut Source::new(pattern(10)), file(&dest, false)).unwrap_err();
    assert!(err.message.contains("exists"), "{}", err.message);
    assert_eq!(std::fs::read_link(&dest).unwrap(), dir.join("nowhere"));
    assert_eq!(names(&dir), ["out"]);
}

// ---------------------------------------------------------------------------
// The shared command line and report
// ---------------------------------------------------------------------------

fn parse(args: &[&str]) -> Result<ExportRequest, CliError> {
    let matches = Cmd::new("export")
        .args(transfer::export_args())
        .try_get_matches_from(args)
        .unwrap();
    ExportRequest::from_matches(&matches)
}

#[test]
fn the_shared_flags_parse_into_a_request() {
    let request = parse(&["export", "/dir/f", "out.bin", "--replace"]).unwrap();
    assert_eq!(request.path, OsString::from("/dir/f"));
    assert_eq!(request.output, Output::File(PathBuf::from("out.bin")));
    assert!(request.replace);

    let request = parse(&["export", "/dir/f", "-"]).unwrap();
    assert_eq!(request.output, Output::Stdout);
    assert!(!request.replace);

    let err = parse(&["export", "/dir/f", "-", "--replace"]).unwrap_err();
    assert_eq!(err.code, output::EXIT_USAGE);
}

#[test]
fn a_file_export_reports_its_bytes_and_digest() {
    let dir = scratch("report");
    let dest = dir.join("out");
    let request = ExportRequest {
        path: OsString::from("/dir/f"),
        output: Output::File(dest.clone()),
        replace: false,
    };
    let data = pattern(CHUNK + 1);
    let outcome = transfer::export(&request, &mut Source::new(data.clone())).unwrap();
    let report = outcome.report.expect("a file export prints a report");
    assert_eq!(report.get("path"), Some(&Json::from("/dir/f")));
    assert_eq!(report.get("bytes"), Some(&Json::from(data.len() as u64)));
    assert_eq!(report.get("replaced"), Some(&Json::from(false)));
    let digest = platform_sha256(std::slice::from_ref(&dest)).remove(0);
    assert_eq!(report.get("sha256"), Some(&Json::from(digest)));
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[test]
fn verified_exports_match_the_platform_digest_including_empty_and_multichunk_files() {
    let dir = scratch("verified");
    for size in [0, 1, CHUNK + 17] {
        let dest = dir.join(format!("out-{size}"));
        let request = ExportRequest {
            path: OsString::from("/f"),
            output: Output::File(dest.clone()),
            replace: false,
        };
        let data = pattern(size);
        let outcome = transfer::export_with_options(
            &request,
            &mut Source::new(data.clone()),
            transfer::ExportOptions { verify: true },
        )
        .unwrap();
        let report = outcome.report.unwrap();
        assert_eq!(report.get("verified"), Some(&Json::from(true)));
        assert_eq!(report.get("bytes"), Some(&Json::from(size as u64)));
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let digest = platform_sha256(std::slice::from_ref(&dest)).remove(0);
        assert_eq!(report.get("sha256"), Some(&Json::from(digest)));
    }
    assert_eq!(names(&dir).len(), 3, "no staging names remain");
}

#[test]
fn verify_parses_and_stdout_is_a_usage_error_before_reading() {
    let matches = Cmd::new("export")
        .args(transfer::export_args_with_verify())
        .try_get_matches_from(["export", "/f", "out", "--verify"])
        .unwrap();
    assert!(
        transfer::ExportOptions::from_matches(&matches)
            .unwrap()
            .verify
    );
    ExportRequest::from_matches(&matches).unwrap();

    let matches = Cmd::new("export")
        .args(transfer::export_args_with_verify())
        .try_get_matches_from(["export", "/f", "-", "--verify"])
        .unwrap();
    let err = ExportRequest::from_matches(&matches).unwrap_err();
    assert_eq!(err.code, output::EXIT_USAGE);
    assert!(err.message.contains("--verify"));

    let mut source = Source::new(b"abc".to_vec());
    let mut sink = Vec::new();
    let err = transfer::export_file_with_options(
        &mut source,
        Destination::Writer(&mut sink),
        transfer::ExportOptions { verify: true },
    )
    .unwrap_err();
    assert_eq!(err.code, output::EXIT_USAGE);
    assert_eq!(source.reads, 0);
    assert!(sink.is_empty());

    let request = ExportRequest {
        path: OsString::from("/f"),
        output: Output::Stdout,
        replace: false,
    };
    let err = transfer::export_with_options(
        &request,
        &mut source,
        transfer::ExportOptions { verify: true },
    )
    .unwrap_err();
    assert_eq!(err.code, output::EXIT_USAGE);
    assert_eq!(source.reads, 0);
}

#[test]
fn an_unverified_export_never_claims_verification() {
    let dir = scratch("unverified");
    let request = ExportRequest {
        path: OsString::from("/f"),
        output: Output::File(dir.join("out")),
        replace: false,
    };
    let outcome = transfer::export(&request, &mut Source::new(b"abc".to_vec())).unwrap();
    assert_eq!(
        outcome.report.unwrap().get("verified"),
        Some(&Json::from(false))
    );
}
