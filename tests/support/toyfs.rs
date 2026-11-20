//! A toy format that implements all three layers, so the traits and the
//! conformance checks can be exercised without a real driver.
//!
//! Layout, in 512-byte blocks:
//!
//! - block 0: the superblock -- `TOYFS\0\0\x01`, block count (u64), root
//!   directory block (u64), a 16-byte label, then a checksum (u32, the byte
//!   sum of bytes 0..40) at 40.
//! - block 1: a copy of the superblock.
//! - directory blocks: `TDIR`, then seven 64-byte entries from byte 64. An
//!   entry is kind (1 file, 2 directory; | 0x80 deleted), name length, the
//!   name (up to 30 bytes), then first block (u64) at 32 and size (u64) at 40.
//!   A directory's size is one block.
//! - file data: contiguous blocks from the entry's first block.

#![allow(dead_code)]

use fs_core::layers::{
    check_bounds, ApiError, Candidate, CandidateKind, CandidateState, Capabilities, ClusterLayout,
    Damage, DamageKind, DamageReport, DirEntry, Encoding, Extent, ExtentWalk, Feature, FileAPI,
    Geometry, Integrity, Metadata, NodeKind, OpenVolume, Parent, ParentClaim, PrimitiveAPI, Probe,
    Recognition, SalvageAdapter, SalvageError, ScanReport, Severity, Sink, Structure,
    StructureRole, SuperblockCopy, Tree, TreeNode, VolumeAPI, VolumeHealth, VolumeSupport, WalkEnd,
    WalkExtent,
};
use fs_core::BlockRead;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::ops::ControlFlow;

pub const BLOCK: u64 = 512;
pub const MAGIC: &[u8; 8] = b"TOYFS\0\0\x01";
const DIR_MAGIC: &[u8; 4] = b"TDIR";
const ENTRY: usize = 64;
const ENTRIES: usize = 7;

/// A deliberate defect, so a test can prove the conformance checks catch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bug {
    /// `read_volume_bytes` serves bytes past the end instead of refusing.
    ReadsPastEnd,
    /// `read_at` returns one byte too few at the start of a file.
    ShortFirstRead,
    /// Capabilities list `ReportDamage` missing, yet the report runs.
    FlagsLie,
    /// The scan reports one candidate more than it streamed.
    ScanMiscounts,
    /// Trees come back in reverse order.
    TreeReversed,
    /// A directory lists a file twice.
    DuplicateEntry,
    /// The health verdict says clean over an error finding.
    HealthHidesDamage,
    /// Encoded extents are walked, but never decoded.
    EncodedUndecoded,
    /// A name that is not UTF-8 is listed by its lossy text, not its bytes.
    LossyBytes,
}

// ---------------------------------------------------------------- image ---

pub struct Builder {
    image: Vec<u8>,
    next: u64,
}

impl Builder {
    pub fn new(blocks: u64) -> Self {
        let mut builder = Self {
            image: vec![0; (blocks * BLOCK) as usize],
            next: 3,
        };
        builder.superblock(0, 2, "TOYVOL");
        builder.superblock(1, 2, "TOYVOL");
        builder.directory(2, 2);
        builder
    }

    fn superblock(&mut self, block: u64, root: u64, label: &str) {
        let blocks = self.image.len() as u64 / BLOCK;
        let at = (block * BLOCK) as usize;
        let sb = &mut self.image[at..at + BLOCK as usize];
        sb[..8].copy_from_slice(MAGIC);
        sb[8..16].copy_from_slice(&blocks.to_le_bytes());
        sb[16..24].copy_from_slice(&root.to_le_bytes());
        sb[24..24 + label.len()].copy_from_slice(label.as_bytes());
        let sum = checksum(&sb[..40]);
        sb[40..44].copy_from_slice(&sum.to_le_bytes());
    }

    fn directory(&mut self, block: u64, parent: u64) {
        let at = (block * BLOCK) as usize;
        self.image[at..at + 4].copy_from_slice(DIR_MAGIC);
        self.image[at + 8..at + 16].copy_from_slice(&parent.to_le_bytes());
    }

    fn entry(&mut self, directory: u64, kind: u8, name: &[u8], first: u64, size: u64) {
        let base = (directory * BLOCK) as usize + ENTRY;
        let slot = (0..ENTRIES)
            .find(|i| self.image[base + i * ENTRY] == 0)
            .expect("directory full");
        let at = base + slot * ENTRY;
        self.image[at] = kind;
        self.image[at + 1] = name.len() as u8;
        self.image[at + 2..at + 2 + name.len()].copy_from_slice(name);
        self.image[at + 32..at + 40].copy_from_slice(&first.to_le_bytes());
        self.image[at + 40..at + 48].copy_from_slice(&size.to_le_bytes());
    }

    /// Adds a file under `directory` and returns its first block.
    pub fn file(&mut self, directory: u64, name: &str, contents: &[u8]) -> u64 {
        self.file_bytes(directory, name.as_bytes(), contents)
    }

    /// A file whose name is stored as these bytes, UTF-8 or not.
    pub fn file_bytes(&mut self, directory: u64, name: &[u8], contents: &[u8]) -> u64 {
        let first = self.next;
        let blocks = (contents.len() as u64).div_ceil(BLOCK).max(1);
        self.next += blocks;
        let at = (first * BLOCK) as usize;
        self.image[at..at + contents.len()].copy_from_slice(contents);
        self.entry(directory, 1, name, first, contents.len() as u64);
        first
    }

    /// A file stored encoded: its bytes reversed, decoded by the `reverse`
    /// codec.
    pub fn encoded_file(&mut self, directory: u64, name: &str, contents: &[u8]) {
        self.encoded_units(directory, name, contents, ONE_UNIT);
    }

    /// A file stored as the encoded units `kind` lays out ([`units`]), back
    /// to back on the source. Each unit decodes to its window of the file
    /// with filler around it, stored reversed, then any padding the unit
    /// stores beyond what it decodes to.
    pub fn encoded_units(&mut self, directory: u64, name: &str, contents: &[u8], kind: u8) {
        let first = self.next;
        let mut at = (first * BLOCK) as usize;
        let mut taken = 0usize;
        for unit in units(kind, contents.len() as u64) {
            let share = &contents[taken..taken + unit.length as usize];
            let mut decoded = vec![0xA7u8; unit.decoded as usize];
            decoded[unit.offset as usize..(unit.offset + unit.length) as usize]
                .copy_from_slice(share);
            decoded.reverse();
            self.image[at..at + decoded.len()].copy_from_slice(&decoded);
            self.image[at + decoded.len()..at + unit.stored as usize].fill(0xEE);
            at += unit.stored as usize;
            taken += unit.length as usize;
        }
        let used = at as u64 - first * BLOCK;
        self.next += used.div_ceil(BLOCK).max(1);
        self.entry(
            directory,
            kind,
            name.as_bytes(),
            first,
            contents.len() as u64,
        );
    }

    pub fn deleted_file(&mut self, directory: u64, name: &str, contents: &[u8]) {
        let first = self.next;
        self.next += (contents.len() as u64).div_ceil(BLOCK).max(1);
        let at = (first * BLOCK) as usize;
        self.image[at..at + contents.len()].copy_from_slice(contents);
        self.entry(
            directory,
            0x81,
            name.as_bytes(),
            first,
            contents.len() as u64,
        );
    }

    /// Adds a directory and returns its block.
    pub fn dir(&mut self, parent: u64, name: &str) -> u64 {
        let block = self.next;
        self.next += 1;
        self.directory(block, parent);
        self.entry(parent, 2, name.as_bytes(), block, BLOCK);
        block
    }

    pub fn build(self) -> Vec<u8> {
        self.image
    }
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().map(|b| u32::from(*b)).sum()
}

/// The image every healthy test uses.
pub fn healthy() -> Vec<u8> {
    let mut builder = Builder::new(64);
    builder.file(2, "hello.txt", b"hello, world\n");
    let big: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
    builder.file(2, "big.bin", &big);
    builder.file(2, "empty", b"");
    let docs = builder.dir(2, "docs");
    builder.file(docs, "readme.md", b"# docs\n");
    let deep = builder.dir(docs, "deep");
    builder.file(deep, "leaf", b"leaf bytes");
    builder.deleted_file(2, "gone.txt", b"deleted but here");
    builder.encoded_file(docs, "packed.txt", b"stored reversed, read forwards");
    // Btrfs's shapes (rust-fs-core#462, #463): a 49-byte file whose encoded
    // unit is 54 bytes, and a file in two encoded units back to back.
    builder.encoded_units(docs, "lzo.txt", &[b'z'; 49], PADDED_UNIT);
    let pair: Vec<u8> = (0..300u32).map(|i| (i % 97) as u8).collect();
    builder.encoded_units(docs, "zstd-pair.bin", &pair, TWO_UNITS);
    // EROFS's shape (rust-fs-erofs#323): a 6-byte tail that is a window of
    // a whole shared fragment block.
    builder.encoded_units(deep, "tail", b"tail6!", FRAGMENT);
    builder.file_bytes(deep, b"caf\xe9", b"latin-1 name");
    builder.build()
}

pub fn big_contents() -> Vec<u8> {
    (0..1500u32).map(|i| (i % 251) as u8).collect()
}

// --------------------------------------------------------------- device ---

/// An in-memory read-only source.
pub struct Memory(pub Vec<u8>);

impl BlockRead for Memory {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> fs_core::Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= self.0.len() as u64)
            .ok_or(fs_core::Error::OutOfBounds {
                offset,
                len: buf.len() as u64,
                size: self.0.len() as u64,
            })?;
        buf.copy_from_slice(&self.0[offset as usize..end as usize]);
        Ok(())
    }

    fn size_bytes(&self) -> u64 {
        self.0.len() as u64
    }
}

// ---------------------------------------------------------------- error ---

#[derive(Debug)]
pub enum ToyError {
    Api(ApiError),
    Device(fs_core::Error),
    Corrupt(&'static str),
}

impl fmt::Display for ToyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(e) => e.fmt(f),
            Self::Device(e) => e.fmt(f),
            Self::Corrupt(what) => write!(f, "corrupt: {what}"),
        }
    }
}

impl std::error::Error for ToyError {}

impl From<ApiError> for ToyError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<fs_core::Error> for ToyError {
    fn from(e: fs_core::Error) -> Self {
        Self::Device(e)
    }
}

// ----------------------------------------------------------- superblock ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    pub blocks: u64,
    pub root: u64,
    pub label: String,
    pub checksum_ok: bool,
}

fn decode_superblock(bytes: &[u8]) -> Option<Superblock> {
    if &bytes[..8] != MAGIC {
        return None;
    }
    let stored = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
    let label = String::from_utf8_lossy(&bytes[24..40])
        .trim_end_matches('\0')
        .to_owned();
    Some(Superblock {
        blocks: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        root: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        label,
        checksum_ok: stored == checksum(&bytes[..40]),
    })
}

fn read_superblock(
    source: &dyn BlockRead,
    copy: u64,
) -> Result<Option<Superblock>, fs_core::Error> {
    let mut bytes = [0u8; BLOCK as usize];
    source.read_at(copy * BLOCK, &mut bytes)?;
    Ok(decode_superblock(&bytes))
}

// --------------------------------------------------------------- volume ---

pub struct ToyVolume {
    source: Memory,
    superblock: Superblock,
    pub bugs: HashSet<Bug>,
}

impl ToyVolume {
    pub fn with_bug(mut self, bug: Bug) -> Self {
        self.bugs.insert(bug);
        self
    }

    fn has(&self, bug: Bug) -> bool {
        self.bugs.contains(&bug)
    }

    fn findings(&self) -> Result<Vec<Damage>, fs_core::Error> {
        let mut findings = Vec::new();
        let primary = read_superblock(&self.source, 0)?;
        let backup = read_superblock(&self.source, 1)?;
        for (copy, superblock) in [(0u64, &primary), (1, &backup)] {
            let location = Extent::new(copy * BLOCK, BLOCK);
            let severity = if copy == 0 {
                Severity::Error
            } else {
                Severity::Warning
            };
            match superblock {
                None => findings.push(Damage {
                    kind: DamageKind::Signature,
                    severity,
                    location,
                    structure: format!("superblock copy {copy}"),
                    detail: "magic missing".into(),
                }),
                Some(sb) if !sb.checksum_ok => findings.push(Damage {
                    kind: DamageKind::Checksum,
                    severity,
                    location,
                    structure: format!("superblock copy {copy}"),
                    detail: "checksum disagrees".into(),
                }),
                Some(_) => {}
            }
        }
        Ok(findings)
    }
}

impl VolumeAPI for ToyVolume {
    type Error = ToyError;
    type Superblock = Superblock;

    fn format(&self) -> &'static str {
        "toyfs"
    }

    fn size_bytes(&self) -> u64 {
        self.source.size_bytes()
    }

    fn read_volume_bytes(&self, offset: u64, output: &mut [u8]) -> Result<(), ToyError> {
        if self.has(Bug::ReadsPastEnd) {
            for (i, byte) in output.iter_mut().enumerate() {
                *byte = self
                    .source
                    .0
                    .get(offset.wrapping_add(i as u64) as usize)
                    .copied()
                    .unwrap_or(0);
            }
            return Ok(());
        }
        check_bounds(offset, output.len(), self.size_bytes())?;
        Ok(self.source.read_at(offset, output)?)
    }

    fn geometry(&self) -> Result<Geometry, ToyError> {
        Ok(Geometry {
            sector_size: BLOCK as u32,
            allocation_unit: BLOCK,
            allocation_units: self.superblock.blocks,
            declared_bytes: self.superblock.blocks * BLOCK,
            layout: Some(ClusterLayout {
                first: 0,
                count: self.superblock.blocks,
                size: BLOCK,
                offset: 0,
            }),
            label: Some(self.superblock.label.clone()),
            serial: None,
        })
    }

    fn superblock(&self) -> Result<Superblock, ToyError> {
        Ok(self.superblock.clone())
    }

    fn superblocks(&self) -> Result<Vec<SuperblockCopy<Superblock>>, ToyError> {
        let mut copies = Vec::new();
        for copy in 0..2u32 {
            let superblock = read_superblock(&self.source, u64::from(copy))?;
            let integrity = match &superblock {
                Some(sb) if sb.checksum_ok => Integrity::Verified,
                _ => Integrity::Failed,
            };
            copies.push(SuperblockCopy {
                copy,
                location: Extent::new(u64::from(copy) * BLOCK, BLOCK),
                superblock,
                integrity,
            });
        }
        Ok(copies)
    }

    fn support(&self) -> Result<VolumeSupport, ToyError> {
        let primary_ok = read_superblock(&self.source, 0)?.is_some_and(|sb| sb.checksum_ok);
        let salvage = PrimitiveAPI::adapter(self)
            .map(|adapter| adapter.capabilities())
            .unwrap_or_default();
        Ok(VolumeSupport {
            files: primary_ok,
            salvage,
            unsupported: Vec::new(),
        })
    }

    fn health(&self) -> Result<VolumeHealth, ToyError> {
        let mut health = VolumeHealth::from_findings(self.findings()?);
        if self.has(Bug::HealthHidesDamage) {
            health.state = fs_core::layers::HealthState::Clean;
        }
        Ok(health)
    }
}

impl OpenVolume for ToyVolume {
    type Source = Memory;

    fn probe(source: &dyn BlockRead) -> Result<Probe, fs_core::Error> {
        let primary = read_superblock(source, 0)?;
        let backup = read_superblock(source, 1)?;
        let (recognition, declared) = match (&primary, &backup) {
            (Some(sb), _) if sb.checksum_ok => (Recognition::Yes, Some(sb.blocks * BLOCK)),
            (_, Some(sb)) => (Recognition::Damaged, Some(sb.blocks * BLOCK)),
            (Some(sb), None) => (Recognition::Damaged, Some(sb.blocks * BLOCK)),
            (None, None) => (Recognition::No, None),
        };
        Ok(Probe {
            format: "toyfs",
            recognition,
            declared_bytes: declared,
            detail: String::new(),
        })
    }

    fn open(source: Memory) -> Result<Self, ToyError> {
        let superblock = read_superblock(&source, 0)?
            .filter(|sb| sb.checksum_ok)
            .ok_or(ToyError::Corrupt("primary superblock"))?;
        Ok(Self {
            source,
            superblock,
            bugs: HashSet::new(),
        })
    }

    fn open_for_salvage(source: Memory) -> Result<Self, ToyError> {
        let superblock = [0, 1]
            .into_iter()
            .filter_map(|copy| read_superblock(&source, copy).ok().flatten())
            .find(|sb| sb.checksum_ok)
            .ok_or(ToyError::Corrupt("every superblock copy"))?;
        Ok(Self {
            source,
            superblock,
            bugs: HashSet::new(),
        })
    }
}

// ----------------------------------------------------------------- file ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    kind: NodeKind,
    first: u64,
    size: u64,
    /// The entry's kind, which says how an encoded file is laid out.
    kind_byte: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawEntry {
    offset: u64,
    kind: u8,
    name: Vec<u8>,
    first: u64,
    size: u64,
}

fn entries_in(source: &dyn BlockRead, block: u64) -> Result<Option<Vec<RawEntry>>, fs_core::Error> {
    let mut bytes = [0u8; BLOCK as usize];
    source.read_at(block * BLOCK, &mut bytes)?;
    if &bytes[..4] != DIR_MAGIC {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for slot in 0..ENTRIES {
        let at = ENTRY + slot * ENTRY;
        let record = &bytes[at..at + ENTRY];
        if record[0] == 0 {
            continue;
        }
        let length = usize::from(record[1]).min(30);
        entries.push(RawEntry {
            offset: block * BLOCK + at as u64,
            kind: record[0],
            name: record[2..2 + length].to_vec(),
            first: u64::from_le_bytes(record[32..40].try_into().unwrap()),
            size: u64::from_le_bytes(record[40..48].try_into().unwrap()),
        });
    }
    Ok(Some(entries))
}

fn kind_of(raw: u8) -> NodeKind {
    if raw & 0x7f == 2 {
        NodeKind::Directory
    } else {
        NodeKind::File
    }
}

impl FileAPI for ToyVolume {
    type Node = Node;

    fn root(&self) -> Result<Node, ToyError> {
        Ok(Node {
            kind: NodeKind::Directory,
            first: self.superblock.root,
            size: BLOCK,
            kind_byte: 2,
        })
    }

    fn read_dir(&self, directory: &Node) -> Result<Vec<DirEntry<Node>>, ToyError> {
        if directory.kind != NodeKind::Directory {
            return Err(ApiError::NotADirectory(format!("block {}", directory.first)).into());
        }
        let entries = entries_in(&self.source, directory.first)?
            .ok_or(ToyError::Corrupt("directory block"))?;
        let mut listed: Vec<DirEntry<Node>> = entries
            .into_iter()
            .filter(|entry| entry.kind & 0x80 == 0)
            .map(|entry| {
                let kind = kind_of(entry.kind);
                let lossy = std::str::from_utf8(&entry.name).is_err();
                DirEntry {
                    name: String::from_utf8_lossy(&entry.name).into_owned(),
                    bytes: if self.has(Bug::LossyBytes) {
                        String::from_utf8_lossy(&entry.name)
                            .into_owned()
                            .into_bytes()
                    } else {
                        entry.name
                    },
                    lossy,
                    kind,
                    size: entry.size,
                    node: Node {
                        kind,
                        first: entry.first,
                        size: entry.size,
                        kind_byte: entry.kind & 0x7f,
                    },
                }
            })
            .collect();
        if self.has(Bug::DuplicateEntry) {
            if let Some(first) = listed.first().cloned() {
                listed.push(first);
            }
        }
        Ok(listed)
    }

    fn metadata(&self, node: &Node) -> Result<Metadata, ToyError> {
        Ok(Metadata::new(node.kind, node.size))
    }

    fn read_at(&self, file: &Node, offset: u64, output: &mut [u8]) -> Result<usize, ToyError> {
        if file.kind == NodeKind::Directory {
            return Err(ApiError::IsADirectory(format!("block {}", file.first)).into());
        }
        if offset >= file.size {
            return Ok(0);
        }
        let mut count = (file.size - offset).min(output.len() as u64) as usize;
        if self.has(Bug::ShortFirstRead) && offset == 0 && count > 1 {
            count -= 1;
        }
        if !units(file.kind_byte, file.size).is_empty() {
            let bytes = decode_file(&self.source, file.first, file.kind_byte, file.size)
                .map_err(|_| ToyError::Corrupt("encoded file"))?;
            output[..count].copy_from_slice(&bytes[offset as usize..offset as usize + count]);
            return Ok(count);
        }
        self.source
            .read_at(file.first * BLOCK + offset, &mut output[..count])?;
        Ok(count)
    }

    fn names_match(&self, stored: &str, wanted: &str) -> bool {
        stored.eq_ignore_ascii_case(wanted)
    }
}

/// One encoded unit holding the whole file.
pub const ONE_UNIT: u8 = 3;
/// Two encoded units, back to back on the source, as Btrfs places
/// consecutive compressed extents.
pub const TWO_UNITS: u8 = 4;
/// One encoded unit stored larger than it decodes to, as LZO stores a short
/// file: five bytes of overhead after the data.
pub const PADDED_UNIT: u8 = 5;

/// A file's tail in a shared fragment block, as EROFS and SquashFS store
/// one: the unit decodes to a whole block of other files' tails, and this
/// file's bytes are a short window of it.
pub const FRAGMENT: u8 = 6;

/// One encoded unit of a file: `stored` bytes on the source that decode to
/// `decoded` bytes, of which `offset..offset + length` are the file's.
#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub stored: u64,
    pub decoded: u64,
    pub offset: u64,
    pub length: u64,
}

impl Unit {
    fn whole(stored: u64, decoded: u64) -> Self {
        Self {
            stored,
            decoded,
            offset: 0,
            length: decoded,
        }
    }
}

/// The encoded units of a `size`-byte file of this kind, in file order;
/// empty for a plain file.
pub fn units(kind: u8, size: u64) -> Vec<Unit> {
    match kind {
        ONE_UNIT => vec![Unit::whole(size, size)],
        TWO_UNITS => {
            let first = size.div_ceil(2);
            vec![
                Unit::whole(first, first),
                Unit::whole(size - first, size - first),
            ]
        }
        PADDED_UNIT => vec![Unit::whole(size + 5, size)],
        FRAGMENT => vec![Unit {
            stored: BLOCK,
            decoded: BLOCK,
            offset: 100,
            length: size,
        }],
        _ => Vec::new(),
    }
}

/// An encoded file's bytes: each unit's decoded share, in order.
fn decode_file(
    source: &dyn BlockRead,
    first: u64,
    kind: u8,
    size: u64,
) -> Result<Vec<u8>, SalvageError> {
    let mut out = Vec::new();
    let mut at = first * BLOCK;
    for unit in units(kind, size) {
        let mut share = vec![0u8; unit.length as usize];
        decode_reversed(
            source,
            Extent::new(at, unit.decoded),
            unit.offset,
            &mut share,
        )?;
        out.extend(share);
        at += unit.stored;
    }
    Ok(out)
}

/// The `reverse` codec: the stored bytes, reversed.
fn decode_reversed(
    source: &dyn BlockRead,
    stored: Extent,
    offset: u64,
    output: &mut [u8],
) -> Result<usize, SalvageError> {
    stored
        .end()
        .filter(|end| *end <= source.size_bytes())
        .ok_or(SalvageError::Bounds)?;
    if offset > stored.length {
        return Err(SalvageError::Bounds);
    }
    let mut bytes = vec![0u8; stored.length as usize];
    source
        .read_at(stored.offset, &mut bytes)
        .map_err(SalvageError::Device)?;
    bytes.reverse();
    let count = (stored.length - offset).min(output.len() as u64) as usize;
    output[..count].copy_from_slice(&bytes[offset as usize..offset as usize + count]);
    Ok(count)
}

// ------------------------------------------------------------ primitive ---

/// The candidate detail: where the entry's data starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToyDetail {
    pub first: u64,
    pub directory: u64,
    /// The entry's kind, which says how an encoded file is laid out.
    pub kind: u8,
    /// The entry's name as stored, which `name` may hold lossily.
    pub name: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ToyAdapter {
    blocks: u64,
    root: u64,
    bugs: HashSet<Bug>,
}

impl SalvageAdapter for ToyAdapter {
    type Detail = ToyDetail;

    fn capabilities(&self) -> Capabilities {
        let mut missing = Vec::new();
        if self.bugs.contains(&Bug::FlagsLie) {
            missing.push(Feature::ReportDamage);
        }
        Capabilities { missing }
    }

    fn find_candidates(
        &self,
        source: &dyn BlockRead,
        range: Extent,
        sink: &mut dyn Sink<Candidate<ToyDetail>>,
    ) -> Result<ScanReport, SalvageError> {
        let end = range
            .end()
            .filter(|end| *end <= source.size_bytes())
            .ok_or(SalvageError::Bounds)?;
        let mut report = ScanReport::default();
        let first_block = range.offset / BLOCK;
        let last_block = end.div_ceil(BLOCK);
        for block in first_block..last_block {
            let entries = match entries_in(source, block) {
                Ok(Some(entries)) => entries,
                Ok(None) => continue,
                Err(_) => {
                    report.unreadable.push(Extent::new(block * BLOCK, BLOCK));
                    continue;
                }
            };
            for entry in entries {
                if entry.offset < range.offset || entry.offset >= end {
                    continue;
                }
                let candidate = Candidate {
                    location: Extent::new(entry.offset, ENTRY as u64),
                    kind: if kind_of(entry.kind) == NodeKind::Directory {
                        CandidateKind::Directory
                    } else {
                        CandidateKind::File
                    },
                    state: if entry.kind & 0x80 != 0 {
                        CandidateState::Deleted
                    } else {
                        CandidateState::Live
                    },
                    integrity: Integrity::Unchecked,
                    name: Some(String::from_utf8_lossy(&entry.name).into_owned()),
                    size: Some(entry.size),
                    detail: ToyDetail {
                        first: entry.first,
                        directory: block,
                        kind: entry.kind & 0x7f,
                        name: entry.name.clone(),
                    },
                };
                report.candidates += 1;
                if sink.accept(candidate) == ControlFlow::Break(()) {
                    report.stopped_at = Some(entry.offset + 1);
                    return Ok(report);
                }
            }
        }
        if self.bugs.contains(&Bug::ScanMiscounts) {
            report.candidates += 1;
        }
        Ok(report)
    }

    fn walk_extents(
        &self,
        source: &dyn BlockRead,
        candidate: &Candidate<ToyDetail>,
    ) -> Result<ExtentWalk, SalvageError> {
        let claimed = candidate.size.unwrap_or(0);
        let start = candidate.detail.first * BLOCK;
        let blocks = claimed.div_ceil(BLOCK);
        if candidate.detail.first + blocks > self.blocks || start + claimed > source.size_bytes() {
            return Ok(ExtentWalk::contiguous(
                Vec::new(),
                claimed,
                WalkEnd::OutOfRange,
            ));
        }
        let encoded = units(candidate.detail.kind, claimed);
        if !encoded.is_empty() && claimed > 0 {
            let mut extents = Vec::new();
            let (mut file_at, mut source_at) = (0u64, start);
            for unit in encoded {
                extents.push(WalkExtent {
                    offset: file_at,
                    source: Extent::new(source_at, unit.stored),
                    uninitialized: false,
                    encoded: Some(Encoding {
                        codec: "reverse".into(),
                        decoded: unit.decoded,
                        offset: unit.offset,
                        length: unit.length,
                    }),
                });
                file_at += unit.length;
                source_at += unit.stored;
            }
            return Ok(ExtentWalk {
                extents,
                claimed,
                end: WalkEnd::Complete,
            });
        }
        let extents = if claimed == 0 {
            Vec::new()
        } else {
            vec![Extent::new(start, claimed)]
        };
        Ok(ExtentWalk::contiguous(extents, claimed, WalkEnd::Complete))
    }

    fn rebuild_tree(
        &self,
        _source: &dyn BlockRead,
        candidates: &[Candidate<ToyDetail>],
    ) -> Result<Tree, SalvageError> {
        let mut nodes: Vec<TreeNode> = candidates
            .iter()
            .map(|candidate| {
                let claim = |parent| ParentClaim {
                    parent,
                    name: Some(candidate.detail.name.clone()),
                    state: candidate.state,
                };
                let parents = if candidate.detail.directory == self.root {
                    vec![claim(Parent::Root)]
                } else {
                    candidates
                        .iter()
                        .filter(|parent| {
                            parent.kind == CandidateKind::Directory
                                && parent.detail.first == candidate.detail.directory
                        })
                        .map(|parent| claim(Parent::Candidate(parent.location.offset)))
                        .collect()
                };
                TreeNode {
                    candidate: candidate.location.offset,
                    parents,
                }
            })
            .collect();
        if self.bugs.contains(&Bug::TreeReversed) {
            nodes.reverse();
        }
        Ok(Tree { nodes })
    }

    fn report_damage(
        &self,
        source: &dyn BlockRead,
        sink: &mut dyn Sink<Damage>,
    ) -> Result<DamageReport, SalvageError> {
        let mut report = DamageReport::default();
        for copy in 0..2u64 {
            let found = read_superblock(source, copy).map_err(SalvageError::Device)?;
            let finding = match found {
                Some(sb) if sb.checksum_ok => continue,
                Some(_) => (DamageKind::Checksum, "checksum disagrees"),
                None => (DamageKind::Signature, "magic missing"),
            };
            report.reported += 1;
            let damage = Damage {
                kind: finding.0,
                severity: if copy == 0 {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                location: Extent::new(copy * BLOCK, BLOCK),
                structure: format!("superblock copy {copy}"),
                detail: finding.1.into(),
            };
            if sink.accept(damage) == ControlFlow::Break(()) {
                report.stopped = true;
                break;
            }
        }
        Ok(report)
    }

    fn read_extent(
        &self,
        source: &dyn BlockRead,
        extent: Extent,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        extent
            .end()
            .filter(|end| *end <= source.size_bytes())
            .ok_or(SalvageError::Bounds)?;
        if offset > extent.length {
            return Err(SalvageError::Bounds);
        }
        let count = (extent.length - offset).min(output.len() as u64) as usize;
        source
            .read_at(extent.offset + offset, &mut output[..count])
            .map_err(SalvageError::Device)?;
        Ok(count)
    }

    fn read_encoded(
        &self,
        source: &dyn BlockRead,
        stored: Extent,
        encoding: &Encoding,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        if encoding.codec != "reverse" || self.bugs.contains(&Bug::EncodedUndecoded) {
            return Err(SalvageError::Unsupported(Feature::EncodedExtents));
        }
        if offset > encoding.length {
            return Err(SalvageError::Bounds);
        }
        // The run is decoded[encoding.offset ..][.. encoding.length].
        let want = (encoding.length - offset).min(output.len() as u64) as usize;
        // The decoded bytes are the first `decoded` stored bytes, reversed;
        // anything stored past them is the unit's overhead.
        if encoding.decoded > stored.length {
            return Err(SalvageError::Bounds);
        }
        decode_reversed(
            source,
            Extent::new(stored.offset, encoding.decoded),
            encoding.offset + offset,
            &mut output[..want],
        )
    }

    fn cluster_layout(&self) -> Result<ClusterLayout, SalvageError> {
        Ok(ClusterLayout {
            first: 0,
            count: self.blocks,
            size: BLOCK,
            offset: 0,
        })
    }
}

impl PrimitiveAPI for ToyVolume {
    type Adapter = ToyAdapter;
    /// The block count, the one thing the toy's geometry needs.
    type Hypothesis = u64;

    fn source(&self) -> &dyn BlockRead {
        &self.source
    }

    fn adapter(&self) -> Result<ToyAdapter, SalvageError> {
        Ok(ToyAdapter {
            blocks: self.superblock.blocks,
            root: self.superblock.root,
            bugs: self.bugs.clone(),
        })
    }

    fn hypotheses(&self) -> Result<Vec<u64>, SalvageError> {
        Ok(vec![self.source.size_bytes() / BLOCK])
    }

    fn adapter_with(&self, blocks: &u64) -> Result<ToyAdapter, SalvageError> {
        Ok(ToyAdapter {
            blocks: *blocks,
            root: 2,
            bugs: self.bugs.clone(),
        })
    }

    fn structures(&self) -> Result<Vec<Structure>, SalvageError> {
        let mut structures = Vec::new();
        for copy in 0..2u32 {
            let ok = read_superblock(&self.source, u64::from(copy))
                .map_err(SalvageError::Device)?
                .is_some_and(|sb| sb.checksum_ok);
            structures.push(Structure {
                name: if copy == 0 {
                    "superblock".into()
                } else {
                    "superblock backup".into()
                },
                role: StructureRole::Superblock,
                location: Extent::new(u64::from(copy) * BLOCK, BLOCK),
                copy,
                integrity: if ok {
                    Integrity::Verified
                } else {
                    Integrity::Failed
                },
            });
        }
        structures.push(Structure {
            name: "root directory".into(),
            role: StructureRole::Directory,
            location: Extent::new(self.superblock.root * BLOCK, BLOCK),
            copy: 0,
            integrity: Integrity::Unchecked,
        });
        Ok(structures)
    }
}
