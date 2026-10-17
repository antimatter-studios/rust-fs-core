//! Import publication is transactional; the standard SHA-256 vector grades bytes.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{self, FileSink, ImportRequest, Input, CHUNK};
use fs_core::cli::CliError;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{self, Read, Write};
use std::path::Path;

struct Counted;
thread_local! {
    static HEAP: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let _ = HEAP.try_with(|h| {
                let (on, live, peak) = h.get();
                if on {
                    h.set((on, live + layout.size(), peak.max(live + layout.size())));
                }
            });
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let _ = HEAP.try_with(|h| {
            let (on, live, peak) = h.get();
            if on {
                h.set((on, live.saturating_sub(layout.size()), peak));
            }
        });
        System.dealloc(p, layout);
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;

#[derive(Default)]
struct Sink {
    old: Option<Vec<u8>>,
    staged: Option<Vec<u8>>,
    atomic: bool,
    fail: &'static str,
    written: u64,
    persisted: bool,
    discard: bool,
}
impl FileSink for Sink {
    fn supports_atomic_replace(&self) -> bool {
        self.atomic
    }
    fn stage(&mut self, _: &Path, replace: bool) -> Result<(), CliError> {
        if !replace && self.old.is_some() {
            return Err(CliError::failed("exists"));
        }
        self.staged = Some(Vec::new());
        Ok(())
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        assert_eq!(offset, self.written);
        assert!(bytes.len() <= CHUNK);
        if self.fail == "write" && offset >= (2 * CHUNK) as u64 {
            return Err(CliError::failed(
                io::Error::from(io::ErrorKind::StorageFull).to_string(),
            ));
        }
        if !self.discard {
            self.staged.as_mut().unwrap().extend_from_slice(bytes);
        }
        self.written += bytes.len() as u64;
        Ok(())
    }
    fn persist(&mut self) -> Result<(), CliError> {
        #[cfg(unix)]
        if self.fail == "signal" {
            assert!(std::process::Command::new("kill")
                .args(["-TERM", &std::process::id().to_string()])
                .status()
                .unwrap()
                .success());
        }
        if self.fail == "persist" {
            return Err(CliError::failed("persist failed"));
        }
        self.persisted = true;
        Ok(())
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> Result<usize, CliError> {
        let mut data = &self.staged.as_ref().unwrap()[offset as usize..];
        data.read(bytes)
            .map_err(|e| CliError::failed(e.to_string()))
    }
    fn publish(&mut self, _: bool) -> Result<bool, CliError> {
        assert!(self.persisted, "publication preceded persistence");
        if self.fail == "publish" {
            return Err(CliError::failed("name appeared"));
        }
        let replaced = self.old.is_some();
        self.old = self.staged.take();
        Ok(replaced)
    }
    fn abort(&mut self) -> Result<(), CliError> {
        self.staged = None;
        if self.fail == "abort" {
            return Err(CliError::failed("cleanup failed"));
        }
        Ok(())
    }
}
struct Producer {
    left: u64,
    fail: bool,
    reads: usize,
}
impl Read for Producer {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        assert!(buf.len() <= CHUNK);
        if self.fail && self.reads > 1 {
            return Err(io::Error::other("source failed"));
        }
        let n = buf.len().min(self.left as usize);
        buf[..n].fill(b'x');
        self.left -= n as u64;
        Ok(n)
    }
}
fn copy(
    source: &mut dyn Read,
    sink: Option<&mut dyn FileSink>,
    replace: bool,
) -> Result<transfer::Copied, CliError> {
    transfer::import_file(source, sink, Path::new("/target"), replace)
}
fn old_sink() -> Sink {
    Sink {
        old: Some(b"old".to_vec()),
        atomic: true,
        ..Sink::default()
    }
}
fn failed_copy(kind: &'static str) {
    let mut sink = old_sink();
    sink.fail = kind;
    let mut source = Producer {
        left: (4 * CHUNK) as u64,
        fail: kind == "source",
        reads: 0,
    };
    let error = copy(&mut source, Some(&mut sink), true).unwrap_err();
    assert_eq!(error.code, 1);
    assert_eq!(sink.old.as_deref(), Some(b"old".as_slice()));
    assert!(sink.staged.is_none());
}
#[test]
fn a_missing_sink_refuses_before_reading_stdin() {
    let mut input = &b"unread"[..];
    assert_eq!(copy(&mut input, None, false).unwrap_err().code, 3);
    assert_eq!(input, b"unread");
}
#[test]
fn source_failure_aborts_without_changing_the_destination() {
    failed_copy("source");
}
#[test]
fn enospc_on_the_third_chunk_aborts_without_changing_the_destination() {
    failed_copy("write");
}
#[test]
fn persistence_failure_aborts_without_changing_the_destination() {
    failed_copy("persist");
}
#[test]
fn publication_failure_aborts_without_changing_the_destination() {
    failed_copy("publish");
}
#[test]
fn no_replace_refuses_an_existing_name_without_reading() {
    let mut input = &b"unread"[..];
    let mut sink = old_sink();
    assert_eq!(
        copy(&mut input, Some(&mut sink), false).unwrap_err().code,
        1
    );
    assert_eq!(input, b"unread");
    assert!(sink.staged.is_none());
    assert_eq!(sink.old.as_deref(), Some(b"old".as_slice()));
}
#[test]
fn unsupported_atomic_replacement_refuses_before_reading() {
    let mut input = &b"unread"[..];
    let mut sink = Sink::default();
    assert_eq!(copy(&mut input, Some(&mut sink), true).unwrap_err().code, 3);
    assert_eq!(input, b"unread");
    assert!(sink.staged.is_none());
}
#[test]
fn created_and_replaced_files_report_the_standard_sha256_vector() {
    for mut sink in [Sink::default(), old_sink()] {
        let replaced = sink.old.is_some();
        let copied = copy(&mut &b"abc"[..], Some(&mut sink), replaced).unwrap();
        assert_eq!(copied.bytes, 3);
        assert_eq!(
            copied.sha256_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(copied.replaced, replaced);
        assert_eq!(sink.old.as_deref(), Some(b"abc".as_slice()));
        assert!(sink.staged.is_none());
    }
}
#[test]
fn an_empty_input_is_persisted_and_published() {
    let mut sink = Sink::default();
    let copied = copy(&mut io::empty(), Some(&mut sink), false).unwrap();
    assert_eq!(copied.bytes, 0);
    assert_eq!(
        copied.sha256_hex(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert!(sink.persisted);
    assert_eq!(sink.old, Some(Vec::new()));
}
#[test]
fn cleanup_failure_is_reported_with_the_original_failure() {
    let mut sink = Sink {
        fail: "abort",
        ..old_sink()
    };
    let mut source = Producer {
        left: (2 * CHUNK) as u64,
        fail: true,
        reads: 0,
    };
    let error = copy(&mut source, Some(&mut sink), true).unwrap_err();
    assert!(error.message.contains("source failed"));
    assert!(error.message.contains("cleanup failed"));
    assert_eq!(sink.old.as_deref(), Some(b"old".as_slice()));
}
#[test]
fn importing_256_mib_holds_under_4_mib_of_heap() {
    let mut sink = Sink {
        discard: true,
        ..Sink::default()
    };
    let mut source = Producer {
        left: 256 << 20,
        fail: false,
        reads: 0,
    };
    HEAP.with(|h| h.set((true, 0, 0)));
    let result = copy(&mut source, Some(&mut sink), false);
    let peak = HEAP.with(|h| {
        let (_, _, peak) = h.replace((false, 0, 0));
        peak
    });
    assert_eq!(result.unwrap().bytes, 256 << 20);
    assert!(peak < 4 << 20, "peak heap growth was {peak}");
}
#[test]
fn import_arguments_accept_stdin_and_default_to_no_replace() {
    let matches = Command::new("import")
        .args(transfer::import_args())
        .try_get_matches_from(["import", "-", "/target"])
        .unwrap();
    let request = ImportRequest::from_matches(&matches).unwrap();
    assert_eq!(request.input, Input::Stdin);
    assert!(!request.replace);
    assert_eq!(request.path, Path::new("/target"));
    assert_eq!(transfer::import(&request, None).unwrap_err().code, 3);
}
#[test]
fn local_files_and_replace_produce_a_structured_report() {
    let local = std::env::temp_dir().join(format!("core-import-{}", std::process::id()));
    std::fs::write(&local, b"abc").unwrap();
    let mut sink = old_sink();
    let request = ImportRequest {
        input: Input::File(local.clone()),
        path: "/target".into(),
        replace: true,
    };
    let result = transfer::import(&request, Some(&mut sink));
    std::fs::remove_file(local).unwrap();
    let report = result.unwrap().report.unwrap().to_compact();
    assert!(report.contains("\"bytes\":3"));
    assert!(report.contains("\"replaced\":true"));
    assert!(report.contains("\"created\":false"));
}

// A real host file is the identity oracle; the reader mutates it between chunks.
struct HookedLocal {
    file: std::fs::File,
    path: std::path::PathBuf,
    hook: Option<fn(&Path)>,
    reads: usize,
}
impl Read for HookedLocal {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.reads == 1 {
            if let Some(hook) = self.hook.take() {
                hook(&self.path);
            }
        }
        self.reads += 1;
        self.file.read(bytes)
    }
}
impl transfer::LocalSource for HookedLocal {
    fn file(&self) -> &std::fs::File {
        &self.file
    }
    fn local_path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}
impl Drop for HookedLocal {
    fn drop(&mut self) {
        // The file may have been removed by the mutation under test.
        if self.path.exists() {
            std::fs::remove_file(&self.path).unwrap();
        }
    }
}
fn local_source(name: &str, hook: Option<fn(&Path)>) -> HookedLocal {
    let path = std::env::temp_dir().join(format!("core-import-{}-{name}", std::process::id()));
    std::fs::write(&path, vec![b'x'; 2 * CHUNK]).unwrap();
    HookedLocal {
        file: std::fs::File::open(&path).unwrap(),
        path,
        hook,
        reads: 0,
    }
}
fn changed_source(name: &str, hook: fn(&Path)) {
    let mut source = local_source(name, Some(hook));
    let mut sink = old_sink();
    let error =
        transfer::import_local_file(&mut source, Some(&mut sink), Path::new("/target"), true)
            .unwrap_err();
    assert_eq!(error.code, 1);
    assert!(
        error.message.contains("source changed during copy"),
        "{error:?}"
    );
    assert_eq!(sink.old.as_deref(), Some(b"old".as_slice()));
    assert!(sink.staged.is_none());
}
#[test]
fn a_growing_local_source_is_not_published() {
    changed_source("growing", |path| {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"new bytes")
            .unwrap();
    });
}
#[test]
fn a_shrinking_local_source_is_not_published() {
    changed_source("shrinking", |path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(CHUNK as u64)
            .unwrap();
    });
}
#[test]
fn a_replaced_local_source_name_is_not_published() {
    changed_source("replaced", |path| {
        std::fs::remove_file(path).unwrap();
        std::fs::write(path, vec![b'x'; 2 * CHUNK]).unwrap();
    });
}
#[test]
fn a_removed_local_source_name_is_not_published() {
    changed_source("removed", |path| std::fs::remove_file(path).unwrap());
}
#[test]
fn an_unchanged_local_source_reports_the_digest_of_the_read_bytes() {
    let mut source = local_source("unchanged", None);
    let mut sink = Sink::default();
    let copied =
        transfer::import_local_file(&mut source, Some(&mut sink), Path::new("/target"), false)
            .unwrap();
    assert_eq!(copied.bytes, (2 * CHUNK) as u64);
    // Compare with an uninterrupted import of the bytes the host exposes.
    let bytes = std::fs::read(&source.path).unwrap();
    let expected = copy(&mut bytes.as_slice(), Some(&mut Sink::default()), false).unwrap();
    assert_eq!(copied.sha256, expected.sha256);
    assert_eq!(sink.old.as_deref(), Some(bytes.as_slice()));
}
#[cfg(unix)]
#[test]
fn a_same_size_rewrite_with_restored_mtime_is_not_published() {
    use std::os::unix::fs::MetadataExt;
    changed_source("restored-mtime", |path| {
        let before = std::fs::metadata(path).unwrap();
        std::fs::write(path, vec![b'y'; 2 * CHUNK]).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        let after = std::fs::metadata(path).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_ne!(
            (before.ctime(), before.ctime_nsec()),
            (after.ctime(), after.ctime_nsec()),
            "the host must expose the rewrite through change time"
        );
    });
}

#[cfg(unix)]
#[test]
fn cancellation_during_persistence_aborts_before_publication() {
    if std::env::var_os("CORE_IMPORT_SIGNAL_CHILD").is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cancellation_during_persistence_aborts_before_publication",
            ])
            .env("CORE_IMPORT_SIGNAL_CHILD", "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        return;
    }
    let mut sink = Sink {
        fail: "signal",
        ..old_sink()
    };
    let error = copy(&mut &b"abc"[..], Some(&mut sink), true).unwrap_err();
    assert_eq!(error.code, 143);
    assert_eq!(sink.old.as_deref(), Some(b"old".as_slice()));
    assert!(sink.staged.is_none());
}
