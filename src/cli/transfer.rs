//! Copying a file out of an image: the `export` verb every `fs.<fs>`
//! shares, written once so that no driver writes it again over a
//! whole-file buffer.
//!
//! A driver supplies a [`FileSource`] — the file's length and a positioned
//! read over its own API — and this module does the rest:
//!
//! - **Bounded.** The copy moves [`CHUNK`] bytes at a time through one
//!   buffer allocated once, so memory does not grow with the file
//!   (`tests/transfer_bounded.rs` measures it).
//! - **Evidence.** Every copy returns its byte count and the SHA-256 of the
//!   bytes delivered, and a file export reports both.
//! - **Complete or not at all.** A local destination is written under a
//!   hidden staging name in its own directory, synced, and only then
//!   published under its real name. A failed or short read, or a failed
//!   write, removes the staging file and leaves the destination as it was.
//!   A crash leaves at most `.<name>.<pid>-<n>.partial` beside it, never a
//!   half-written destination.
//! - **Never over a file unasked.** Without `--replace` an existing name —
//!   a file, a directory or a symlink, dangling or not — is refused before
//!   the image is read, and publication is a hard link, which fails rather
//!   than overwrite a name that appeared meanwhile. There is no fallback to
//!   check-then-rename: a filesystem without hard links is told to use
//!   `--replace`. With it, publication is a rename, which swaps the new file
//!   in atomically; a directory is still refused.
//! - **Missing data is never success.** A writer that fails (a closed pipe)
//!   fails the copy, saying how many bytes of the total were delivered.
//!
//! Opt-in verification reopens the synced staging file and compares its
//! length and SHA-256 before publication. Drivers supplying
//! [`FileSource::image_path`] also refuse local destinations that identify
//! the backing image, before reading bytes or creating staging files.
//! Trees and imports can call [`refuse_image_alias`] for each local path.
//! Sources supplying an image path also hold a shared [`ImageLock`] while
//! exporting; a competing writer produces a warning before the first read.
//!
//! On Unix, SIGINT and SIGTERM cancel at chunk boundaries and before file
//! publication. Staging is removed by normal code; repeated signals cannot
//! bypass cleanup. Low-level copies return status 130/143 in `CliError`;
//! command exports return a cancellation report (stderr for raw stdout copies).

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches};
use sha2::{Digest, Sha256};

use super::output::{CliError, Json, Outcome};

#[path = "transfer_lock.rs"]
mod image_lock;
pub use image_lock::ImageLock;

#[path = "transfer_progress.rs"]
mod progress;
pub use progress::{export_args_with_progress, Progress, ProgressOptions};

#[path = "transfer_cancel.rs"]
mod cancel;
use cancel::Cancellation;

#[path = "transfer_import.rs"]
mod import;
pub use import::{import, import_args, import_file, FileSink, ImportRequest, Input};

#[path = "transfer_tree.rs"]
mod tree;
pub use tree::{
    export_args_with_links, export_args_with_times, export_args_with_tree, export_tree,
    export_tree_with_links, export_tree_with_progress, export_tree_with_times, LinkOptions,
    SymbolicLink, TimeOptions, TreeEntry, TreeKind, TreeMetadata, TreeOptions, TreeSource,
};

#[cfg(test)]
#[path = "transfer_verify_tests.rs"]
mod verify_tests;

/// The bytes a transfer moves at a time. One buffer of this size (or of
/// the file's size, when smaller) is all the file data a copy holds.
pub const CHUNK: usize = 1 << 20;

/// A file a driver can read at a position: what `export` copies from.
pub trait FileSource {
    /// The backing image or device, before applying any partition offset.
    /// Supply this to protect file exports against image aliases. Existing
    /// sources default to no identity; drivers should override this when
    /// their source has a local backing path.
    fn image_path(&self) -> Option<&Path> {
        None
    }

    /// The file's length in bytes, read once before the copy starts.
    fn size(&self) -> u64;

    /// Read into `buf` from `offset`, returning how many bytes were placed
    /// there. Fewer than asked is allowed; none before [`size`](Self::size)
    /// is a short read, and fails the copy. An error is the driver's own,
    /// and is passed on as it is.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError>;
}

/// Where an export's bytes go.
pub enum Destination<'a> {
    /// A writer, such as stdout: bytes go out as they are read.
    Writer(&'a mut dyn Write),
    /// A local file, published only when complete.
    File {
        /// The name to publish.
        path: &'a Path,
        /// Replace an existing file rather than refuse it.
        replace: bool,
    },
}

/// What a copy delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// The bytes delivered.
    pub bytes: u64,
    /// The SHA-256 of those bytes.
    pub sha256: [u8; 32],
    /// Whether an existing local file was replaced.
    pub replaced: bool,
}

impl Copied {
    /// The digest in lowercase hex, as checksum tools print it.
    pub fn sha256_hex(&self) -> String {
        self.sha256.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Optional export behavior. Existing entry points use the default, which
/// copies without read-back verification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Reopen the synced staging file and verify its length and SHA-256
    /// before publishing. Writers, including stdout, cannot be verified.
    pub verify: bool,
}

impl ExportOptions {
    /// Parse the options from [`export_args_with_verify`]. Matches from
    /// the original [`export_args`] also work, with verification disabled.
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, CliError> {
        let verify = matches
            .try_get_one::<bool>("verify")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false);
        if verify
            && matches
                .get_one::<OsString>("dest")
                .is_some_and(|d| d == "-")
        {
            return Err(CliError::usage("--verify needs a file DEST, not stdout"));
        }
        Ok(Self { verify })
    }
}

/// Copy `source` to `destination`. See the module documentation for what
/// is promised about memory, publication and failures.
pub fn export_file(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
) -> Result<Copied, CliError> {
    export_file_with_options(source, destination, ExportOptions::default())
}

/// Copy with optional staging-file verification. Verification of a writer
/// is a usage error, returned before reading any source bytes.
pub fn export_file_with_options(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
    options: ExportOptions,
) -> Result<Copied, CliError> {
    let mut stderr = io::stderr();
    let terminal = stderr.is_terminal();
    let mut progress = Progress::new(&mut stderr, terminal, ProgressOptions::default());
    progress.set_totals(Some(source.size()), Some(1));
    let name = match &destination {
        Destination::Writer(_) => PathBuf::from("-"),
        Destination::File { path, .. } => path.to_path_buf(),
    };
    export_file_with_progress(source, destination, options, &name, &mut progress)
}

/// Export with a shared stderr reporter. Set its overall totals before copying;
/// unknown totals stay unknown. Completion is reported only after publication.
pub fn export_file_with_progress(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
    options: ExportOptions,
    name: &Path,
    progress: &mut Progress<'_>,
) -> Result<Copied, CliError> {
    let cancellation = Cancellation::install()?;
    export_file_cancellable(source, destination, options, name, progress, &cancellation)
}

fn export_file_cancellable(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
    options: ExportOptions,
    name: &Path,
    progress: &mut Progress<'_>,
    cancellation: &Cancellation,
) -> Result<Copied, CliError> {
    progress.begin_file(name, Some(source.size()));
    let result = copy_file(source, destination, options, progress, cancellation);
    progress.finish_file(result.is_ok());
    result
}

fn copy_file(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
    options: ExportOptions,
    progress: &mut Progress<'_>,
    cancellation: &Cancellation,
) -> Result<Copied, CliError> {
    cancellation.check(0, source.size())?;
    if matches!(&destination, Destination::Writer(_)) && options.verify {
        return Err(CliError::usage("--verify needs a file DEST, not a writer"));
    }
    let _image_lock = source
        .image_path()
        .map(|image| ImageLock::for_export(image, &mut io::stderr().lock()))
        .transpose()?;
    match destination {
        Destination::Writer(sink) => pump(source, sink, &mut |n| progress.advance(n), cancellation),
        Destination::File { path, replace } => export_to_file_observed(
            source,
            path,
            replace,
            options.verify,
            &mut |n| progress.advance(n),
            cancellation,
            |file, _| file.sync_all(),
        ),
    }
}

/// The copy itself: chunk by chunk from `source` into `sink`, digesting
/// what passes.
fn pump(
    source: &mut dyn FileSource,
    sink: &mut dyn Write,
    update: &mut dyn FnMut(u64),
    cancellation: &Cancellation,
) -> Result<Copied, CliError> {
    let size = source.size();
    let mut buf = vec![0u8; usize::try_from(size).map_or(CHUNK, |s| s.min(CHUNK))];
    let mut digest = Sha256::new();
    let mut offset = 0u64;
    let delivered = |offset: u64, e: io::Error| {
        CliError::failed(format!(
            "output failed with {offset} of {size} bytes delivered: {e}"
        ))
    };
    while offset < size {
        cancellation.check(offset, size)?;
        let want = buf
            .len()
            .min(usize::try_from(size - offset).unwrap_or(usize::MAX));
        let read = source.read_at(offset, &mut buf[..want]);
        cancellation.check(offset, size)?;
        let got = read?;
        if got == 0 {
            return Err(CliError::failed(format!(
                "short read: the source ended at {offset} of {size} bytes"
            )));
        }
        if got > want {
            return Err(CliError::failed(format!(
                "the source answered {got} bytes when {want} were asked for at byte {offset}"
            )));
        }
        digest.update(&buf[..got]);
        let mut remaining = &buf[..got];
        while !remaining.is_empty() {
            let written = match sink.write(remaining) {
                Ok(0) => return Err(delivered(offset, io::ErrorKind::WriteZero.into())),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    cancellation.check(offset, size)?;
                    continue;
                }
                result => result.map_err(|e| delivered(offset, e))?,
            };
            remaining = &remaining[written..];
            offset += written as u64;
            update(offset);
        }
    }
    cancellation.check(offset, size)?;
    let flushed = sink.flush();
    cancellation.check(offset, size)?;
    flushed.map_err(|e| delivered(offset, e))?;
    Ok(Copied {
        bytes: offset,
        sha256: digest.finalize().into(),
        replaced: false,
    })
}

#[cfg(test)]
fn export_to_file(
    source: &mut dyn FileSource,
    path: &Path,
    replace: bool,
    verify: bool,
    sync: impl FnOnce(&mut File, &Path) -> io::Result<()>,
) -> Result<Copied, CliError> {
    export_to_file_observed(
        source,
        path,
        replace,
        verify,
        &mut |_| {},
        &Cancellation::install()?,
        sync,
    )
}

fn export_to_file_observed(
    source: &mut dyn FileSource,
    path: &Path,
    replace: bool,
    verify: bool,
    update: &mut dyn FnMut(u64),
    cancellation: &Cancellation,
    sync: impl FnOnce(&mut File, &Path) -> io::Result<()>,
) -> Result<Copied, CliError> {
    if let Some(image) = source.image_path() {
        refuse_image_alias(image, path)?;
    }
    let name = path
        .file_name()
        .ok_or_else(|| CliError::usage(format!("{}: not a file name to create", path.display())))?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    // Everything that can be refused is refused before the image is read.
    let existing = fs::symlink_metadata(path).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(CliError::failed(format!(
                "{}: is a directory",
                path.display()
            )));
        }
        if !replace {
            return Err(exists(path));
        }
    }
    if !fs::metadata(parent).is_ok_and(|m| m.is_dir()) {
        return Err(CliError::failed(format!(
            "{}: no such directory to export into",
            parent.display()
        )));
    }

    let (staging, mut file) = create_staging(parent, name)?;
    let copied = pump(source, &mut file, update, cancellation).and_then(|copied| {
        let synced = sync(&mut file, &staging);
        cancellation.check(copied.bytes, copied.bytes)?;
        synced.map_err(|e| CliError::failed(format!("sync {}: {e}", staging.display())))?;
        Ok(copied)
    });
    drop(file);
    let published = copied.and_then(|copied| {
        if verify {
            verify_staging(&staging, &copied, cancellation)?;
        }
        cancellation.check(copied.bytes, copied.bytes)?;
        publish(&staging, path, replace)?;
        Ok(copied)
    });
    let mut copied = match published {
        Ok(copied) => copied,
        Err(e) => {
            let _ = fs::remove_file(&staging);
            return Err(e);
        }
    };
    sync_dir(parent).map_err(|e| {
        CliError::failed(format!(
            "{} is complete, but its directory could not be synced: {e}",
            path.display()
        ))
    })?;
    copied.replaced = existing.is_some();
    Ok(copied)
}

/// Refuse a local transfer source or destination that identifies `image`.
/// Call this for each local path in an import or directory-tree transfer,
/// before creating anything or moving bytes. Symlinks are followed; hard
/// links compare by file identity, and Unix device nodes by device number.
/// Partition offsets do not change the backing path's identity.
///
/// A missing local path is allowed, including a dangling destination
/// symlink. Other identity errors fail closed. Errors name both paths and
/// have exit code 1. This is a preflight check, not protection against a
/// concurrent process changing paths after the check.
pub fn refuse_image_alias(image: &Path, local: &Path) -> Result<(), CliError> {
    let failed = |e| {
        CliError::failed(format!(
            "cannot compare image {} with local path {}: {e}",
            image.display(),
            local.display()
        ))
    };
    let image_meta = fs::metadata(image).map_err(failed)?;
    let local_meta = match fs::metadata(local) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(failed(e)),
    };
    #[cfg(unix)]
    let same = {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let same_node =
            image_meta.dev() == local_meta.dev() && image_meta.ino() == local_meta.ino();
        let same_device_kind = (image_meta.file_type().is_block_device()
            && local_meta.file_type().is_block_device())
            || (image_meta.file_type().is_char_device() && local_meta.file_type().is_char_device());
        same_node || (same_device_kind && image_meta.rdev() == local_meta.rdev())
    };
    #[cfg(not(unix))]
    let same = {
        let _ = (image_meta, local_meta);
        same_file::is_same_file(image, local).map_err(failed)?
    };
    if same {
        return Err(CliError::failed(format!(
            "image {} and local path {} identify the same file or device",
            image.display(),
            local.display()
        )));
    }
    Ok(())
}

/// Read until EOF, including any unexpected trailing bytes. The original
/// copy buffer has already been dropped, so verification holds one chunk.
fn verify_staging(
    staging: &Path,
    expected: &Copied,
    cancellation: &Cancellation,
) -> Result<(), CliError> {
    let failed = |e| CliError::failed(format!("verify {}: {e}", staging.display()));
    let mut file = File::open(staging).map_err(failed)?;
    let mut buf = vec![0u8; CHUNK];
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    loop {
        cancellation.check(bytes, expected.bytes)?;
        let n = match file.read(&mut buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(failed)?,
        };
        if n == 0 {
            break;
        }
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| CliError::failed("verification byte count overflow"))?;
        digest.update(&buf[..n]);
    }
    let actual = Copied {
        bytes,
        sha256: digest.finalize().into(),
        replaced: false,
    };
    if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
        return Err(CliError::failed(format!(
            "verification failed for {}: expected {} bytes, sha256 {}; read back {} bytes, sha256 {}",
            staging.display(),
            expected.bytes,
            expected.sha256_hex(),
            actual.bytes,
            actual.sha256_hex()
        )));
    }
    Ok(())
}

fn exists(path: &Path) -> CliError {
    CliError::failed(format!(
        "{}: exists; give --replace to replace it",
        path.display()
    ))
}

/// A new, empty file under a hidden name beside the destination. The
/// name is never one that exists: it is created exclusively, and the next
/// candidate is tried when it is taken.
fn create_staging(parent: &Path, name: &OsStr) -> Result<(PathBuf, File), CliError> {
    // The stem only has to be recognisable; it is shortened so that a
    // destination name near the length limit still leaves room for the
    // suffix.
    let stem: String = name.to_string_lossy().chars().take(100).collect();
    let pid = std::process::id();
    for n in 0..1000u32 {
        let staging = parent.join(format!(".{stem}.{pid}-{n}.partial"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => return Ok((staging, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(CliError::failed(format!(
                    "create {}: {e}",
                    staging.display()
                )))
            }
        }
    }
    Err(CliError::failed(format!(
        "{}: no free staging name beside it",
        parent.join(name).display()
    )))
}

/// Give the complete staging file its real name.
fn publish(staging: &Path, path: &Path, replace: bool) -> Result<(), CliError> {
    if replace {
        return fs::rename(staging, path)
            .map_err(|e| CliError::failed(format!("replace {}: {e}", path.display())));
    }
    match fs::hard_link(staging, path) {
        Ok(()) => fs::remove_file(staging).map_err(|e| {
            CliError::failed(format!(
                "{} is published, but its staging name {} could not be removed: {e}",
                path.display(),
                staging.display()
            ))
        }),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(exists(path)),
        Err(e) => Err(CliError::failed(format!(
            "{}: cannot publish without replacing, which needs a hard link: {e}; \
             --replace publishes by rename instead",
            path.display()
        ))),
    }
}

/// Make the new name durable: on Unix a rename or link is only on disk
/// once its directory is synced.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// The shared command line
// ---------------------------------------------------------------------------

/// Where an `export` command line sends the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// `-`: the bytes go to stdout, and nothing else does.
    Stdout,
    /// A local file to create (or, with `--replace`, to replace).
    File(PathBuf),
}

/// An `export` command line, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    /// The file inside the image, as given.
    pub path: OsString,
    /// Where it goes.
    pub output: Output,
    /// `--replace`.
    pub replace: bool,
}

/// The arguments every driver's `export` takes, so they all parse alike:
/// `PATH DEST [--replace]`, DEST `-` meaning stdout.
pub fn export_args() -> [Arg; 3] {
    [
        Arg::new("path")
            .value_name("PATH")
            .help("The file inside the image")
            .required(true)
            .value_parser(clap::value_parser!(OsString)),
        Arg::new("dest")
            .value_name("DEST")
            .help("The local file to create, or - for stdout")
            .required(true)
            .value_parser(clap::value_parser!(OsString)),
        Arg::new("replace")
            .long("replace")
            .action(ArgAction::SetTrue)
            .help(
                "Replace DEST if it exists, atomically; without this an existing DEST is refused",
            ),
    ]
}

/// The original export arguments plus `--verify`. Drivers opting in parse
/// [`ExportOptions::from_matches`] and call [`export_with_options`]. The
/// original argument array and request fields remain unchanged.
pub fn export_args_with_verify() -> [Arg; 4] {
    let [path, dest, replace] = export_args();
    [
        path,
        dest,
        replace,
        Arg::new("verify")
            .long("verify")
            .action(ArgAction::SetTrue)
            .help("Read the synced staging file back and verify its length and SHA-256 before publishing"),
    ]
}

impl ExportRequest {
    /// The request [`export_args`] parsed. `--replace` with stdout is a
    /// usage error: there is nothing there to replace.
    pub fn from_matches(matches: &ArgMatches) -> Result<ExportRequest, CliError> {
        let given = |id: &str| {
            matches
                .get_one::<OsString>(id)
                .cloned()
                .ok_or_else(|| CliError::usage(format!("missing {}", id.to_uppercase())))
        };
        let path = given("path")?;
        let dest = given("dest")?;
        let replace = matches.get_flag("replace");
        ExportOptions::from_matches(matches)?;
        let output = if dest == "-" {
            if replace {
                return Err(CliError::usage("--replace needs a file DEST, not stdout"));
            }
            Output::Stdout
        } else {
            Output::File(PathBuf::from(dest))
        };
        Ok(ExportRequest {
            path,
            output,
            replace,
        })
    }
}

/// Run an `export` the driver has opened `source` for. To stdout, the
/// bytes are the whole output; to a file, the result is a report of the
/// bytes and their digest.
pub fn export(request: &ExportRequest, source: &mut dyn FileSource) -> Result<Outcome, CliError> {
    export_with_options(request, source, ExportOptions::default())
}

/// Export with optional read-back verification. A successful file report
/// carries `verified: true` only after the staged bytes matched the copy.
pub fn export_with_options(
    request: &ExportRequest,
    source: &mut dyn FileSource,
    options: ExportOptions,
) -> Result<Outcome, CliError> {
    let mut stderr = io::stderr();
    let terminal = stderr.is_terminal();
    let mut progress = Progress::new(&mut stderr, terminal, ProgressOptions::default());
    export_with_progress(request, source, options, &mut progress)
}

/// Export content and its final report with progress confined to the reporter.
/// Construct it from stderr and [`ProgressOptions::from_matches`].
pub fn export_with_progress(
    request: &ExportRequest,
    source: &mut dyn FileSource,
    options: ExportOptions,
    progress: &mut Progress<'_>,
) -> Result<Outcome, CliError> {
    let cancellation = Cancellation::install()?;
    progress.set_totals(Some(source.size()), Some(1));
    let dest = match &request.output {
        Output::Stdout if options.verify => {
            return Err(CliError::usage("--verify needs a file DEST, not stdout"));
        }
        Output::Stdout => {
            let mut stdout = io::stdout().lock();
            let result = export_file_cancellable(
                source,
                Destination::Writer(&mut stdout),
                options,
                Path::new(&request.path),
                progress,
                &cancellation,
            );
            if let Err(error) = result {
                if !cancel::is_cancelled(&error) {
                    return Err(error);
                }
                let outcome = cancel::cancelled_file(request, error);
                // Stdout contains raw content, so its cancellation report is
                // diagnostic output, never appended to the content stream.
                if let Some(report) = outcome.report {
                    let _ = writeln!(io::stderr(), "{}", report.to_compact());
                }
                return Ok(Outcome::done().with_code(outcome.code));
            }
            return Ok(Outcome::done());
        }
        Output::File(dest) => dest,
    };
    let copied = match export_file_cancellable(
        source,
        Destination::File {
            path: dest,
            replace: request.replace,
        },
        options,
        Path::new(&request.path),
        progress,
        &cancellation,
    ) {
        Ok(copied) => copied,
        Err(error) if cancel::is_cancelled(&error) => {
            return Ok(cancel::cancelled_file(request, error))
        }
        Err(error) => return Err(error),
    };
    let path = request.path.to_string_lossy();
    let report = Json::object([
        ("path", Json::from(path.as_ref())),
        ("destination", Json::from(dest.display().to_string())),
        ("bytes", Json::from(copied.bytes)),
        ("sha256", Json::from(copied.sha256_hex())),
        ("replaced", Json::from(copied.replaced)),
        ("verified", Json::from(options.verify)),
    ]);
    let text = format!(
        "exported {path} to {} ({} bytes, sha256 {})",
        dest.display(),
        copied.bytes,
        copied.sha256_hex()
    );
    Ok(Outcome::report(report).with_text(text))
}
