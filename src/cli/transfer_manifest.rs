//! Durable verified-file evidence, shared by transfer command adapters.
//!
//! This storage layer does not infer verification or capture identities. Callers
//! must supply identities of the actual transferred objects, append only after
//! read-back verification and publication, and report [`ManifestRead::torn_tail`].
//! Adapters use [`Manifest::open_for_export`] or [`Manifest::open_for_import`]
//! before copying to reject image/local aliases. Each append syncs before
//! returning; an error stops the transfer.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::super::output::CliError;
use clap::{Arg, ArgAction, ArgMatches};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Bound individual record buffers during serialization and recovery.
const MAX_LINE: usize = 64 * 1024;

/// Opt-in export ledger arguments, separate from existing request literals.
/// Parsing performs no I/O. Execution adapters must open the ledger through
/// [`Manifest::open_for_export`] before transfer I/O and apply resume decisions
/// with locked, fresh endpoint identities; ordinary export APIs do not do this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportManifestOptions {
    /// Local JSONL ledger path, retained without lossy Unicode conversion.
    pub manifest: Option<PathBuf>,
    /// Skip only files supported by revalidated evidence of earlier verification.
    pub resume: bool,
}

impl ExportManifestOptions {
    /// Parse [`export_args_with_manifest`] or [`export_args_with_resume`].
    /// Legacy argument sets select no ledger and no resume. A custom parser
    /// must still supply a manifest with resume and use a local destination.
    /// Manifest recording without `--verify` is accepted but records no entries.
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, CliError> {
        let manifest = matches
            .try_get_one::<PathBuf>("manifest")
            .ok()
            .flatten()
            .cloned();
        let resume = matches
            .try_get_one::<bool>("resume")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false);
        if resume && manifest.is_none() {
            return Err(CliError::usage("--resume requires --manifest FILE"));
        }
        if manifest.is_some()
            && matches
                .try_get_one::<OsString>("dest")
                .ok()
                .flatten()
                .is_some_and(|dest| dest == "-")
        {
            return Err(CliError::usage("--manifest needs a file DEST, not stdout"));
        }
        Ok(Self { manifest, resume })
    }
}

/// All tree and metadata arguments plus an opt-in local manifest path.
/// Drivers must consume [`ExportManifestOptions`] in their ledger adapter;
/// adding these arguments alone does not enable recording or resume execution.
pub fn export_args_with_manifest() -> [Arg; 8] {
    let [path, dest, replace, verify, recursive, depth, preserve] =
        super::export_args_with_xattrs();
    [
        path,
        dest,
        replace,
        verify,
        recursive,
        depth,
        preserve,
        Arg::new("manifest")
            .long("manifest")
            .value_name("FILE")
            .value_parser(clap::value_parser!(PathBuf))
            .help("Record verified, published files in a local JSONL ledger; requires a file DEST"),
    ]
}

/// Manifest export arguments with evidence-based resume. `--resume` requires
/// `--manifest FILE`; endpoint and digest validation belongs to the adapter.
/// Earlier verification never means destination bytes were verified now.
pub fn export_args_with_resume() -> [Arg; 9] {
    let [path, dest, replace, verify, recursive, depth, preserve, manifest] =
        export_args_with_manifest();
    [
        path, dest, replace, verify, recursive, depth, preserve, manifest,
        Arg::new("resume")
            .long("resume")
            .action(ArgAction::SetTrue)
            .requires("manifest")
            .help("Skip files verified earlier only when manifest identities and a fresh source digest match"),
    ]
}

/// A signed Unix timestamp, including subsecond precision and pre-epoch times.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    /// Whole seconds since the Unix epoch, rounded down.
    pub seconds: i64,
    /// Nanoseconds within that second; less than one billion.
    pub nanoseconds: u32,
}

/// Identity captured from an opened local file, without lossy path conversion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalIdentity {
    /// Local path associated with the opened object.
    pub path: PathBuf,
    /// Host device or volume identifier.
    pub device: u64,
    /// Host inode or file index.
    pub inode: u64,
    /// File length at capture.
    pub size: u64,
    /// Host access time.
    pub accessed: Timestamp,
    /// Host modification time.
    pub modified: Timestamp,
    /// Host metadata change time.
    pub changed: Timestamp,
}

/// A local file or a file within a filesystem image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Identity {
    /// A local endpoint.
    Local(LocalIdentity),
    /// An image endpoint, including its backing object's identity.
    Image {
        /// Path within the image filesystem.
        path: PathBuf,
        /// Backing image or device identity, before partition offsets.
        image: LocalIdentity,
        /// Byte offset of the filesystem in the image.
        offset: u64,
        /// Filesystem UUID supplied by the driver.
        filesystem_uuid: String,
        /// Filesystem inode supplied by the driver.
        inode: u64,
    },
}

/// Evidence for one completed, verified file; directories and attempts are absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Identity of the object whose bytes were read.
    pub source: Identity,
    /// Identity of the published destination object.
    pub destination: Identity,
    /// Number of verified bytes.
    pub bytes: u64,
    /// SHA-256 of those bytes, as 64 lowercase hexadecimal digits.
    pub sha256: String,
    /// Completion time, captured after verification and publication.
    pub completed_at: Timestamp,
}

/// Previously completed records and crash recovery information.
#[derive(Debug)]
pub struct ManifestRead {
    /// Valid newline-terminated records, in append order.
    pub entries: Vec<ManifestEntry>,
    /// An unterminated last line was ignored and durably removed. The caller
    /// must report this to the user even when it contained otherwise valid JSON.
    pub torn_tail: bool,
}

/// Whether a file needs a complete copy or has evidence of earlier verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeDecision {
    /// Copy from byte zero, including any partial or changed file.
    Copy,
    /// Report `skipped: verified earlier`, with this original completion time.
    /// This is not a verification of destination bytes performed now.
    Skipped {
        /// Time recorded after the earlier verification and publication.
        completed_at: Timestamp,
    },
}

impl ManifestRead {
    /// Check the latest evidence for this destination against fresh identities
    /// and a SHA-256 streamed from the source's beginning through EOF.
    /// Missing or changed evidence requires a complete copy; read errors fail.
    /// Memory is bounded by the transfer chunk size, irrespective of file size.
    /// Access time is excluded because reading a file can change it.
    ///
    /// The caller must capture identities from the actual opened endpoints,
    /// retain transfer locks, and revalidate those snapshots before applying a
    /// skip. This storage API cannot detect concurrent endpoint mutations.
    /// It neither appends evidence nor marks anything as verified now. Report
    /// `torn_tail` separately, even when this decision requires copying.
    pub fn check_resume(
        &self,
        source: &Identity,
        destination: &Identity,
        reader: &mut dyn Read,
    ) -> Result<ResumeDecision, CliError> {
        // A later transfer from another source supersedes the old destination
        // evidence too. Never search backwards for an older matching digest.
        let Some(entry) = self
            .entries
            .iter()
            .rev()
            .find(|entry| same_location(&entry.destination, destination))
        else {
            return Ok(ResumeDecision::Copy);
        };
        let failed = |e| CliError::failed(format!("resume source digest: {e}"));
        validate(entry).map_err(failed)?;
        let size_matches = |identity: &Identity| match identity {
            Identity::Local(local) => local.size == entry.bytes,
            Identity::Image { .. } => true,
        };
        if !same_identity(&entry.source, source)
            || !same_identity(&entry.destination, destination)
            || !size_matches(source)
            || !size_matches(destination)
        {
            return Ok(ResumeDecision::Copy);
        }
        let mut buffer = vec![0; super::CHUNK];
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        loop {
            let n = match reader.read(&mut buffer) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => result.map_err(failed)?,
            };
            if n == 0 {
                break;
            }
            if n > buffer.len() {
                return Err(failed(io::Error::other("reader exceeded the buffer")));
            }
            let Some(total) = bytes.checked_add(n as u64) else {
                return Ok(ResumeDecision::Copy);
            };
            bytes = total;
            if bytes > entry.bytes {
                return Ok(ResumeDecision::Copy);
            }
            digest.update(&buffer[..n]);
        }
        if bytes != entry.bytes || format!("{:x}", digest.finalize()) != entry.sha256 {
            return Ok(ResumeDecision::Copy);
        }
        Ok(ResumeDecision::Skipped {
            completed_at: entry.completed_at,
        })
    }
}

fn same_location(a: &Identity, b: &Identity) -> bool {
    match (a, b) {
        (Identity::Local(a), Identity::Local(b)) => a.path == b.path,
        (
            Identity::Image {
                path: a,
                image: ai,
                offset: ao,
                ..
            },
            Identity::Image {
                path: b,
                image: bi,
                offset: bo,
                ..
            },
        ) => a == b && ai.path == bi.path && ao == bo,
        _ => false,
    }
}

fn same_local(a: &LocalIdentity, b: &LocalIdentity) -> bool {
    a.path == b.path
        && a.device == b.device
        && a.inode == b.inode
        && a.size == b.size
        && a.modified == b.modified
        && a.changed == b.changed
}

fn same_identity(a: &Identity, b: &Identity) -> bool {
    match (a, b) {
        (Identity::Local(a), Identity::Local(b)) => same_local(a, b),
        (
            Identity::Image {
                path: a,
                image: ai,
                offset: ao,
                filesystem_uuid: au,
                inode: an,
            },
            Identity::Image {
                path: b,
                image: bi,
                offset: bo,
                filesystem_uuid: bu,
                inode: bn,
            },
        ) => a == b && same_local(ai, bi) && ao == bo && au == bu && an == bn,
        _ => false,
    }
}

/// An exclusively locked JSONL ledger. Keep it open for the whole transfer.
/// Concurrent writers fail immediately, rather than interleave evidence.
#[derive(Debug)]
pub struct Manifest {
    file: File,
    path: PathBuf,
    healthy: bool,
}

#[cfg(unix)]
impl Drop for Manifest {
    fn drop(&mut self) {
        // A concurrent fork retains the open description until exec, even
        // with CLOEXEC. Closing only our descriptor can leave its lock held.
        let _ = self.file.unlock();
    }
}

impl Manifest {
    /// Preflight an import ledger before creating it or recovering any contents.
    /// Reject image aliases, the source itself, paths inside a source tree, and
    /// hard/symbolic links to existing source files. Set `source_is_tree` for
    /// recursive imports. The source must exist; identity errors fail closed.
    /// Uses the same safe opener as [`Self::open_for_export`], including its
    /// ancestor resolution and refusal to traverse directory symlinks.
    ///
    /// Call before reading source contents or staging image writes, and keep the
    /// ledger open throughout the import. Callers must exclude concurrent path
    /// mutation and report the returned recovery information as for [`Self::open`].
    pub fn open_for_import(
        path: &Path,
        image: &Path,
        source: &Path,
        source_is_tree: bool,
    ) -> Result<(Self, ManifestRead), CliError> {
        std::fs::metadata(source).map_err(|e| {
            CliError::failed(format!(
                "manifest preflight source {}: {e}",
                source.display()
            ))
        })?;
        Self::open_for_export(path, image, source, source_is_tree)
    }

    /// Preflight an export ledger before creating it or recovering any contents.
    /// Reject image aliases, the destination itself, paths inside a destination
    /// tree, and hard/symbolic links to existing destination files. Set
    /// `destination_is_tree` for recursive exports, including a missing root.
    /// Existing directory symlinks are not traversed, as tree exports refuse
    /// them. Host identity and enumeration errors fail closed.
    ///
    /// Call before any transfer reads or destination writes, and keep the
    /// returned ledger open throughout the export. Callers must exclude
    /// concurrent path mutation; this preflight is not a filesystem snapshot.
    /// The returned recovery information must be reported just as for [`Self::open`].
    pub fn open_for_export(
        path: &Path,
        image: &Path,
        destination: &Path,
        destination_is_tree: bool,
    ) -> Result<(Self, ManifestRead), CliError> {
        super::refuse_image_alias(image, path)?;
        let failed = |e| CliError::failed(format!("manifest preflight {}: {e}", path.display()));
        let resolved = resolve_export_path(path).map_err(failed)?;
        let dest = resolve_export_path(destination).map_err(failed)?;
        if resolved == dest || (destination_is_tree && resolved.starts_with(&dest)) {
            return Err(failed(io::Error::other(
                "manifest aliases or is inside the local transfer path",
            )));
        }
        match std::fs::metadata(path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(failed(io::Error::other("not a regular manifest file")));
            }
            Ok(_) => {
                // A missing manifest cannot already share an inode. For an
                // existing one, compare host identities before recovery can
                // truncate a torn tail in an aliased exported file.
                if destination_is_tree {
                    refuse_tree_alias(path, destination)?;
                } else {
                    super::refuse_image_alias(path, destination)?;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(failed(e)),
        }
        Self::open(path)
    }

    /// Open or exclusively create a ledger without truncating completed records.
    /// Validate every completed line before repairing a torn final line; invalid
    /// completed JSON/schema/digests fail without modifying existing contents.
    /// Parent directories must already exist. New directory entries are synced
    /// on hosts supporting directory sync, as for local transfer publication.
    pub fn open(path: &Path) -> Result<(Self, ManifestRead), CliError> {
        let failed = |e| CliError::failed(format!("manifest {}: {e}", path.display()));
        let (file, created) = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => (file, true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(failed)?,
                false,
            ),
            Err(e) => return Err(failed(e)),
        };
        super::image_lock::lock(&file, true).map_err(|e| failed(io::Error::other(e)))?;
        if !file.metadata().map_err(failed)?.is_file() {
            return Err(failed(io::Error::other("not a regular manifest file")));
        }
        let mut manifest = Self {
            file,
            path: path.into(),
            healthy: true,
        };
        let read = manifest.recover().map_err(failed)?;
        if created {
            manifest.file.sync_all().map_err(failed)?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            super::sync_dir(parent).map_err(failed)?;
        }
        Ok((manifest, read))
    }

    fn recover(&mut self) -> io::Result<ManifestRead> {
        let mut result = ManifestRead {
            entries: Vec::new(),
            torn_tail: false,
        };
        let mut valid_end = 0u64;
        let mut reader = BufReader::new(&mut self.file);
        loop {
            let mut line = Vec::new();
            (&mut reader)
                .take((MAX_LINE + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if line.is_empty() {
                break;
            }
            if line.len() > MAX_LINE {
                return Err(io::Error::other("manifest line exceeds 64 KiB"));
            }
            if line.last() != Some(&b'\n') {
                result.torn_tail = true;
                break;
            }
            let entry: ManifestEntry = serde_json::from_slice(&line).map_err(io::Error::other)?;
            validate(&entry)?;
            valid_end += line.len() as u64;
            result.entries.push(entry);
        }
        if result.torn_tail {
            self.file.set_len(valid_end)?;
            self.file.sync_all()?;
        }
        self.file.seek(SeekFrom::End(0))?;
        Ok(result)
    }

    /// Append only when the caller confirms successful read-back verification
    /// and publication. `false` leaves the ledger unchanged and returns `false`.
    /// A successful `true` return means the complete JSON line has been synced.
    /// A write/sync failure poisons this handle: stop copying and reopen to
    /// recover rather than appending behind a potentially torn record.
    pub fn append(&mut self, entry: &ManifestEntry, verified: bool) -> Result<bool, CliError> {
        if !verified {
            return Ok(false);
        }
        let failed = |e| CliError::failed(format!("manifest {}: {e}", self.path.display()));
        if !self.healthy {
            return Err(failed(io::Error::other(
                "previous append failed; reopen to recover",
            )));
        }
        validate(entry).map_err(failed)?;
        let mut line = serde_json::to_vec(entry).map_err(|e| failed(io::Error::other(e)))?;
        line.push(b'\n');
        if line.len() > MAX_LINE {
            return Err(failed(io::Error::other("manifest line exceeds 64 KiB")));
        }
        self.healthy = false;
        self.file
            .write_all(&line)
            .and_then(|()| self.file.sync_all())
            .map_err(failed)?;
        self.healthy = true;
        Ok(true)
    }
}

// Resolve each existing component before processing `..`, so a symlink's
// parent is its target's parent. Missing suffixes still participate in lexical
// containment checks; dangling symlinks and other resolution errors fail closed.
fn resolve_export_path(path: &Path) -> io::Result<PathBuf> {
    use std::path::Component;
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::canonicalize(&resolved) {
                    Ok(canonical) => resolved = canonical,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        match std::fs::symlink_metadata(&resolved) {
                            Err(missing) if missing.kind() == io::ErrorKind::NotFound => {}
                            _ => return Err(e),
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

fn refuse_tree_alias(manifest: &Path, destination: &Path) -> Result<(), CliError> {
    let failed = |e| {
        CliError::failed(format!(
            "manifest preflight tree {}: {e}",
            destination.display()
        ))
    };
    let metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(failed(e)),
    };
    super::refuse_image_alias(manifest, destination)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for entry in std::fs::read_dir(destination).map_err(failed)? {
            refuse_tree_alias(manifest, &entry.map_err(failed)?.path())?;
        }
    }
    Ok(())
}

fn validate(entry: &ManifestEntry) -> io::Result<()> {
    let times = |identity: &Identity| {
        let local = match identity {
            Identity::Local(local) => local,
            Identity::Image { image, .. } => image,
        };
        [local.accessed, local.modified, local.changed]
    };
    if entry.sha256.len() != 64
        || !entry
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::Error::other("invalid lowercase SHA-256 digest"));
    }
    if times(&entry.source)
        .into_iter()
        .chain(times(&entry.destination))
        .chain([entry.completed_at])
        .any(|time| time.nanoseconds >= 1_000_000_000)
    {
        return Err(io::Error::other("invalid timestamp nanoseconds"));
    }
    Ok(())
}
