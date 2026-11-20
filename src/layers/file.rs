//! FileAPI: browse, read and copy files on a volume whose namespace is
//! intact.
//!
//! A driver implements four methods -- [`FileAPI::root`],
//! [`FileAPI::read_dir`], [`FileAPI::metadata`] and [`FileAPI::read_at`] --
//! over a node handle of its own choosing. Path lookup and copying are
//! provided on top of them, so every driver resolves paths and copies files
//! the same way.

use super::volume::{ApiError, VolumeAPI};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;

/// Bytes [`FileAPI::copy_to`] reads per call.
const COPY_CHUNK: usize = 256 * 1024;

/// What kind of object a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
    /// A device node, FIFO, socket or other special file.
    Special,
}

/// A time as seconds and nanoseconds since the Unix epoch, UTC. A format
/// with local times or coarser units converts as best it can and says so in
/// its documentation.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

/// A node's metadata, in format-neutral terms. Fields a format does not
/// record are `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub kind: NodeKind,
    /// The logical length in bytes; 0 for a directory where the format has
    /// no meaningful length for one.
    pub size: u64,
    /// Bytes allocated on the volume, where the format records it.
    pub allocated: Option<u64>,
    pub created: Option<Timestamp>,
    pub modified: Option<Timestamp>,
    pub accessed: Option<Timestamp>,
    /// Unix permission bits, where the format records them.
    pub mode: Option<u32>,
    /// Hard link count, where the format records it.
    pub links: Option<u64>,
    /// The format's own attribute bits (FAT and NTFS attributes, ext4 flags).
    pub attributes: Option<u64>,
}

impl Metadata {
    /// Metadata with only a kind and a size.
    pub fn new(kind: NodeKind, size: u64) -> Self {
        Self {
            kind,
            size,
            allocated: None,
            created: None,
            modified: None,
            accessed: None,
            mode: None,
            links: None,
            attributes: None,
        }
    }
}

/// One entry of a directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry<N> {
    /// The name, decoded from the format's encoding. Never empty, never
    /// `.` or `..`, and never holding `/` or NUL.
    pub name: String,
    /// The name as a byte string: exactly the stored bytes on a format whose
    /// names are bytes (ext4, XFS, Btrfs, bcachefs, EROFS, SquashFS), and the
    /// UTF-8 of `name` on one that stores a text encoding (FAT, exFAT, NTFS).
    /// [`FileAPI::lookup_bytes`] matches it, so an entry whose name is not
    /// UTF-8 can still be found by path.
    pub bytes: Vec<u8>,
    /// True when `bytes` did not decode and `name` holds replacement
    /// characters. Such an entry is found by [`FileAPI::lookup_bytes`], not by
    /// [`FileAPI::lookup`].
    pub lossy: bool,
    pub kind: NodeKind,
    /// The logical size, as the directory records it or as the node's
    /// metadata gives it.
    pub size: u64,
    /// The handle for [`FileAPI::metadata`], [`FileAPI::read_at`] and, for a
    /// directory, [`FileAPI::read_dir`].
    pub node: N,
}

/// The top layer: the namespace of a healthy volume.
///
/// Paths are absolute, `/`-separated and start at the root; empty components
/// are ignored, so `/`, `` and `//` all name the root. Name comparison is
/// [`FileAPI::names_match`], exact by default and overridden by
/// case-insensitive formats.
pub trait FileAPI: VolumeAPI {
    /// The driver's handle on a file or directory: an inode number, an MFT
    /// reference with its sequence, a directory entry's location. It belongs
    /// to the volume that issued it.
    type Node: Clone + fmt::Debug;

    /// The root directory.
    fn root(&self) -> Result<Self::Node, Self::Error>;

    /// Lists a directory, without `.` and `..`. Fails with
    /// [`ApiError::NotADirectory`] on anything else.
    fn read_dir(&self, directory: &Self::Node) -> Result<Vec<DirEntry<Self::Node>>, Self::Error>;

    fn metadata(&self, node: &Self::Node) -> Result<Metadata, Self::Error>;

    /// Reads file bytes from `offset` and returns how many were read: exactly
    /// the smaller of the bytes left in the file and the output's length, so
    /// 0 at or past the end and never a short read before it. Fails with
    /// [`ApiError::IsADirectory`] on a directory.
    fn read_at(
        &self,
        file: &Self::Node,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, Self::Error>;

    /// A symbolic link's target, as stored. Defaults to
    /// [`ApiError::Unsupported`] for formats without links.
    fn read_link(&self, link: &Self::Node) -> Result<Vec<u8>, Self::Error> {
        let _ = link;
        Err(ApiError::Unsupported("symbolic links").into())
    }

    /// Whether a stored name matches a wanted one. Exact by default; a
    /// case-insensitive format overrides it with its own folding.
    fn names_match(&self, stored: &str, wanted: &str) -> bool {
        stored == wanted
    }

    /// Whether a stored byte name matches a wanted one: through
    /// [`Self::names_match`] when both are UTF-8, so a format's case folding
    /// still applies, and byte for byte otherwise.
    fn names_match_bytes(&self, stored: &[u8], wanted: &[u8]) -> bool {
        match (std::str::from_utf8(stored), std::str::from_utf8(wanted)) {
            (Ok(stored), Ok(wanted)) => self.names_match(stored, wanted),
            _ => stored == wanted,
        }
    }

    /// Resolves a path from the root, one [`Self::read_dir`] per component.
    /// Symbolic links are not followed. The same as [`Self::lookup_bytes`]
    /// over the path's UTF-8.
    fn lookup(&self, path: &str) -> Result<Self::Node, Self::Error> {
        self.lookup_bytes(path.as_bytes())
    }

    /// Resolves a byte path from the root: `/`-separated, empty components
    /// ignored, each matched against [`DirEntry::bytes`] through
    /// [`Self::names_match_bytes`]. A name that is not UTF-8 is found this
    /// way. Symbolic links are not followed.
    fn lookup_bytes(&self, path: &[u8]) -> Result<Self::Node, Self::Error> {
        let mut node = self.root()?;
        let mut walked = Vec::new();
        for component in path
            .split(|byte| *byte == b'/')
            .filter(|part| !part.is_empty())
        {
            if self.metadata(&node)?.kind != NodeKind::Directory {
                let shown = String::from_utf8_lossy(&walked).into_owned();
                return Err(ApiError::NotADirectory(shown).into());
            }
            let entries = self.read_dir(&node)?;
            walked.push(b'/');
            walked.extend_from_slice(component);
            node = entries
                .into_iter()
                .find(|entry| self.names_match_bytes(&entry.bytes, component))
                .ok_or_else(|| ApiError::NotFound(String::from_utf8_lossy(&walked).into_owned()))?
                .node;
        }
        Ok(node)
    }

    /// Copies a file's bytes to `sink` and returns how many were copied:
    /// exactly the file's size, or an error.
    fn copy_to(&self, file: &Self::Node, sink: &mut dyn io::Write) -> Result<u64, Self::Error> {
        let size = self.metadata(file)?.size;
        let mut buffer = vec![0u8; COPY_CHUNK];
        let mut copied = 0u64;
        while copied < size {
            let want = (size - copied).min(COPY_CHUNK as u64) as usize;
            let read = self.read_at(file, copied, &mut buffer[..want])?;
            if read == 0 {
                return Err(ApiError::Bounds {
                    offset: copied,
                    length: want as u64,
                    size,
                }
                .into());
            }
            sink.write_all(&buffer[..read]).map_err(ApiError::Sink)?;
            copied += read as u64;
        }
        Ok(copied)
    }
}
