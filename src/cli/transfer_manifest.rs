//! Durable verified-file evidence, shared by transfer command adapters.
//!
//! This storage layer does not infer verification or capture identities. Callers
//! must supply identities of the actual transferred objects, append only after
//! read-back verification and publication, and report [`ManifestRead::torn_tail`].
//! Command adapters must preflight the manifest against image/tree aliases before
//! opening it. Each append syncs before returning; an error stops the transfer.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::super::output::CliError;
use serde::{Deserialize, Serialize};

// Bound individual record buffers during serialization and recovery.
const MAX_LINE: usize = 64 * 1024;

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

/// An exclusively locked JSONL ledger. Keep it open for the whole transfer.
/// Concurrent writers fail immediately, rather than interleave evidence.
#[derive(Debug)]
pub struct Manifest {
    file: File,
    path: PathBuf,
    healthy: bool,
}

impl Manifest {
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
