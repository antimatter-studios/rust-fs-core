//! Host allocation and host SHA-256 independently grade sparse transfers.
#![cfg(feature = "cli")]
use fs_core::cli::clap::Command;
use fs_core::cli::transfer::{
    self, Destination, ExportOptions, FileSink, FileSource, SparseOptions, CHUNK,
};
use fs_core::cli::CliError;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("core-sparse-{}-{tag}", std::process::id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Source(Vec<u8>);
impl FileSource for Source {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        let mut data = &self.0[offset as usize..];
        data.read(buf).map_err(|e| CliError::failed(e.to_string()))
    }
}
fn data() -> Vec<u8> {
    let mut data = vec![0; 6 * CHUNK + 17];
    data[..CHUNK].fill(19);
    data[3 * CHUNK..4 * CHUNK].fill(37);
    data
}
fn sparse() -> SparseOptions {
    SparseOptions {
        preserve_sparse: true,
    }
}
struct Sink {
    file: File,
    supported: bool,
    written: u64,
    length: Option<u64>,
    published: bool,
    aborted: bool,
    fail_length: bool,
}
impl Sink {
    fn new(p: &Path) -> Self {
        Self {
            file: File::create(p).unwrap(),
            supported: true,
            written: 0,
            length: None,
            published: false,
            aborted: false,
            fail_length: false,
        }
    }
}
impl FileSink for Sink {
    fn supports_sparse(&self) -> bool {
        self.supported
    }
    fn set_sparse_len(&mut self, len: u64) -> Result<(), CliError> {
        if self.fail_length {
            return Err(CliError::failed("length failed"));
        }
        self.file.set_len(len).unwrap();
        self.length = Some(len);
        Ok(())
    }
    fn stage(&mut self, _: &Path, _: bool) -> Result<(), CliError> {
        Ok(())
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), CliError> {
        self.file.seek(SeekFrom::Start(offset)).unwrap();
        self.file.write_all(bytes).unwrap();
        self.written += bytes.len() as u64;
        Ok(())
    }
    fn persist(&mut self) -> Result<(), CliError> {
        self.file.sync_all().unwrap();
        Ok(())
    }
    fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize, CliError> {
        unreachable!()
    }
    fn publish(&mut self, _: bool) -> Result<bool, CliError> {
        self.published = true;
        Ok(false)
    }
    fn abort(&mut self) -> Result<(), CliError> {
        self.aborted = true;
        Ok(())
    }
}
fn import(sink: &mut Sink, bytes: Vec<u8>, options: SparseOptions) -> transfer::Copied {
    transfer::import_file_with_sparse(
        &mut Cursor::new(bytes),
        Some(sink),
        Path::new("/file"),
        false,
        options,
    )
    .unwrap()
}
fn export(p: &Path, bytes: Vec<u8>, options: SparseOptions) -> transfer::Copied {
    transfer::export_file_with_sparse(
        &mut Source(bytes),
        Destination::File {
            path: p,
            replace: false,
        },
        ExportOptions { verify: true },
        options,
    )
    .unwrap()
}
#[test]
fn sparse_combines_with_existing_preserve_policies() {
    for args in [
        transfer::export_args_with_times().to_vec(),
        transfer::import_args_with_tree().to_vec(),
    ] {
        let m = Command::new("copy")
            .args(args)
            .try_get_matches_from([
                "copy",
                "src",
                "dest",
                "--preserve=times,sparse",
                "--preserve=links",
            ])
            .unwrap();
        assert!(SparseOptions::from_matches(&m).unwrap().preserve_sparse);
        assert!(
            transfer::TimeOptions::from_matches(&m)
                .unwrap()
                .preserve_times
        );
        assert!(
            transfer::LinkOptions::from_matches(&m)
                .unwrap()
                .preserve_links
        );
    }
    let m = Command::new("copy")
        .args(transfer::export_args())
        .try_get_matches_from(["copy", "src", "dest"])
        .unwrap();
    assert_eq!(
        SparseOptions::from_matches(&m).unwrap(),
        SparseOptions::default()
    );
}
#[test]
fn sparse_import_skips_zero_chunks_and_finishes_the_trailing_hole() {
    let dir = Scratch::new("import");
    let p = dir.0.join("file");
    let mut sink = Sink::new(&p);
    let copied = import(&mut sink, data(), sparse());
    assert_eq!(sink.written, (2 * CHUNK) as u64);
    assert_eq!(sink.length, Some(copied.bytes));
    assert!(sink.published && !sink.aborted);
    assert_eq!(fs::read(p).unwrap(), data());
}
#[test]
fn dense_import_writes_every_byte_without_sparse_capability() {
    let dir = Scratch::new("dense");
    let mut sink = Sink::new(&dir.0.join("file"));
    sink.supported = false;
    let copied = import(&mut sink, data(), SparseOptions::default());
    assert_eq!(sink.written, copied.bytes);
    assert_eq!(sink.length, None);
}
#[test]
fn unsupported_sparse_import_refuses_before_consuming_input() {
    let dir = Scratch::new("refuse");
    let mut sink = Sink::new(&dir.0.join("file"));
    sink.supported = false;
    let mut source = Cursor::new(data());
    let error = transfer::import_file_with_sparse(
        &mut source,
        Some(&mut sink),
        Path::new("/file"),
        false,
        sparse(),
    )
    .unwrap_err();
    assert_eq!(error.code, 3);
    assert!(error.message.contains("sparse"));
    assert_eq!(source.position(), 0);
    assert!(!sink.published);
}
#[test]
fn failed_sparse_length_aborts_before_persistence_and_publication() {
    let dir = Scratch::new("length-failed");
    let mut sink = Sink::new(&dir.0.join("file"));
    sink.fail_length = true;
    let error = transfer::import_file_with_sparse(
        &mut Cursor::new(vec![0; CHUNK + 13]),
        Some(&mut sink),
        Path::new("/file"),
        false,
        sparse(),
    )
    .unwrap_err();
    assert!(error.message.contains("length failed"));
    assert!(sink.aborted && !sink.published);
}
#[test]
fn empty_and_all_zero_sparse_files_keep_their_logical_length() {
    let dir = Scratch::new("zero");
    for len in [0, CHUNK + 17] {
        let p = dir.0.join(format!("export-{len}"));
        let exported = export(&p, vec![0; len], sparse());
        let imported_path = dir.0.join(format!("import-{len}"));
        let mut sink = Sink::new(&imported_path);
        let imported = import(&mut sink, vec![0; len], sparse());
        assert_eq!(exported.sha256, imported.sha256);
        assert_eq!(fs::metadata(p).unwrap().len(), len as u64);
        assert_eq!(fs::read(imported_path).unwrap(), vec![0; len]);
        assert_eq!(sink.written, 0);
    }
}
#[test]
fn writer_destinations_still_receive_all_zero_bytes() {
    let bytes = vec![0; CHUNK + 19];
    let mut output = Vec::new();
    let copied = transfer::export_file_with_sparse(
        &mut Source(bytes.clone()),
        Destination::Writer(&mut output),
        ExportOptions::default(),
        sparse(),
    )
    .unwrap();
    assert_eq!(copied.bytes, bytes.len() as u64);
    assert_eq!(output, bytes);
}
#[cfg(unix)]
fn digest(p: &Path) -> String {
    let result = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(p)
        .output()
        .or_else(|_| std::process::Command::new("sha256sum").arg(p).output())
        .expect("install shasum or sha256sum for the independent digest oracle");
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}
// APFS may allocate small seek gaps (including data()'s two-MiB gaps).
// Keep that fixture for byte/write-count checks, and grade physical holes
// with the same larger gaps on every Unix host, against native file I/O.
#[cfg(unix)]
const HOST_GAP: usize = 32 * CHUNK;
#[cfg(unix)]
fn host_data() -> Vec<u8> {
    let mut bytes = vec![0; 2 * HOST_GAP + 2 * CHUNK + 17];
    bytes[..CHUNK].fill(19);
    bytes[HOST_GAP + CHUNK..HOST_GAP + 2 * CHUNK].fill(37);
    bytes
}
#[cfg(unix)]
fn native_sparse(p: &Path) -> fs::Metadata {
    use std::os::unix::fs::MetadataExt;
    let mut file = File::create(p).unwrap();
    file.write_all(&vec![19; CHUNK]).unwrap();
    file.seek(SeekFrom::Start((HOST_GAP + CHUNK) as u64))
        .unwrap();
    file.write_all(&vec![37; CHUNK]).unwrap();
    file.set_len((2 * HOST_GAP + 2 * CHUNK + 17) as u64)
        .unwrap();
    file.sync_all().unwrap();
    let metadata = file.metadata().unwrap();
    assert!(
        metadata.blocks() * 512 < metadata.len(),
        "native seek/truncate did not retain the oracle fixture's holes: {metadata:?}"
    );
    metadata
}
#[cfg(unix)]
#[test]
fn sparse_export_uses_fewer_host_blocks_with_the_same_host_digest() {
    use std::os::unix::fs::MetadataExt;
    let dir = Scratch::new("export-oracle");
    // The original small file must still copy exactly, even when the host
    // chooses to allocate its unwritten gaps.
    let small = dir.0.join("small");
    let copied = export(&small, data(), sparse());
    assert_eq!(fs::read(&small).unwrap(), data());
    assert_eq!(copied.sha256_hex(), digest(&small));
    let native = dir.0.join("native");
    let expected = native_sparse(&native);
    let dense = dir.0.join("dense");
    let holes = dir.0.join("holes");
    let a = export(&dense, host_data(), SparseOptions::default());
    let b = export(&holes, host_data(), sparse());
    let m = fs::metadata(&holes).unwrap();
    assert!(
        m.blocks() * 512 < m.len(),
        "host did not retain holes: {m:?}"
    );
    assert!(m.blocks() < fs::metadata(&dense).unwrap().blocks());
    assert_eq!(m.len(), expected.len());
    assert!(m.blocks() <= expected.blocks());
    assert_eq!(digest(&native), digest(&holes));
    assert_eq!(digest(&dense), digest(&holes));
    assert_eq!(b.sha256_hex(), digest(&holes));
    assert_eq!(a, b);
}
#[cfg(unix)]
#[test]
fn sparse_import_uses_host_holes_and_reports_the_host_digest() {
    use std::os::unix::fs::MetadataExt;
    let dir = Scratch::new("import-oracle");
    let native = dir.0.join("native");
    let expected = native_sparse(&native);
    let dense = dir.0.join("dense");
    fs::write(&dense, host_data()).unwrap();
    let p = dir.0.join("file");
    let mut sink = Sink::new(&p);
    let copied = import(&mut sink, host_data(), sparse());
    let m = fs::metadata(&p).unwrap();
    assert!(
        m.blocks() * 512 < m.len(),
        "host did not retain holes: {m:?}"
    );
    assert!(m.blocks() < fs::metadata(&dense).unwrap().blocks());
    assert_eq!(m.len(), expected.len());
    assert!(m.blocks() <= expected.blocks());
    assert_eq!(digest(&native), digest(&p));
    assert_eq!(digest(&dense), digest(&p));
    assert_eq!(copied.sha256_hex(), digest(&p));
}
