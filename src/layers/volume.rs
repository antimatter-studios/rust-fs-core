//! VolumeAPI: open a volume, read its bytes, and report its geometry, its
//! boot sector or superblock, and its health.
//!
//! This is the bottom layer. It reads the source and decodes the structures
//! that say what the volume is; it does not walk the namespace. A volume
//! opened for salvage stays usable through this layer when the namespace is
//! damaged, so [`PrimitiveAPI`](super::PrimitiveAPI) can work beneath it.

use super::salvage::{Capabilities, ClusterLayout, Damage, Extent, Integrity, Severity};
use crate::BlockRead;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;

/// A refusal that every layer shares, so provided methods and a generic
/// consumer can name one without knowing the driver's own error type. Each
/// driver's `Error` converts from it.
#[derive(Debug)]
#[non_exhaustive]
pub enum ApiError {
    /// The driver does not implement this operation; the text names it.
    Unsupported(&'static str),
    /// No object at this path.
    NotFound(String),
    /// A path component, or the node asked to list, is not a directory.
    NotADirectory(String),
    /// The node asked to read as a file is a directory.
    IsADirectory(String),
    /// A read names bytes outside the volume, or an unrepresentable range.
    Bounds { offset: u64, length: u64, size: u64 },
    /// The caller's output sink failed.
    Sink(io::Error),
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(formatter, "unsupported: {what}"),
            Self::NotFound(path) => write!(formatter, "not found: {path}"),
            Self::NotADirectory(path) => write!(formatter, "not a directory: {path}"),
            Self::IsADirectory(path) => write!(formatter, "is a directory: {path}"),
            Self::Bounds {
                offset,
                length,
                size,
            } => write!(
                formatter,
                "{length} byte(s) at {offset} lie outside the {size}-byte volume"
            ),
            Self::Sink(error) => write!(formatter, "output sink failed: {error}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sink(error) => Some(error),
            _ => None,
        }
    }
}

/// The format-neutral shape of a volume, decoded from its boot sector or
/// superblock. Format-specific fields stay in the driver's
/// [`VolumeAPI::Superblock`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    /// The sector or device block size the format records, in bytes. A format
    /// that records none (squashfs, erofs) gives 1 or its read granularity.
    pub sector_size: u32,
    /// The unit the format allocates file data in -- cluster, block, extent
    /// unit -- in bytes.
    pub allocation_unit: u64,
    /// How many allocation units the format addresses.
    pub allocation_units: u64,
    /// The volume length the format records, in bytes. It may differ from
    /// [`VolumeAPI::size_bytes`]: a truncated image records more than it has.
    pub declared_bytes: u64,
    /// The linear numbering of allocation units on the source, when the
    /// format has one. `None` for a format whose addresses map through a
    /// table (btrfs chunks, bcachefs) rather than a fixed offset.
    pub layout: Option<ClusterLayout>,
    /// The volume label, when the format stores one.
    pub label: Option<String>,
    /// The volume serial number or UUID, as text, when the format stores one.
    pub serial: Option<String>,
}

/// The volume's overall state, as its own structures describe it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum HealthState {
    /// The structures checked agree and record no fault.
    Clean,
    /// The format records that it was not cleanly unmounted, or that a
    /// journal or log must be replayed. Nothing checked is damaged.
    NeedsRecovery,
    /// Everything checked is usable, but a redundant copy is damaged: a
    /// broken backup superblock, a mirror that disagrees with its primary.
    /// Nothing is lost yet; one more failure would lose it.
    Degraded,
    /// A checked structure is damaged; see the findings.
    Damaged,
    /// The driver cannot tell, for example because it opened the volume
    /// without decoding the structures that would say.
    Unknown,
}

/// The volume's health: a state and the findings behind it.
///
/// This is the quick verdict taken when a volume opens -- boot copies,
/// superblock checksums, descriptor tables, dirty flags, the root -- and it
/// can miss deep damage, such as a zeroed subdirectory block. `Clean` means
/// the structures checked here are sound, not that the volume holds no
/// damage. A full scan of every structure is
/// [`SalvageAdapter::report_damage`](super::SalvageAdapter::report_damage).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeHealth {
    pub state: HealthState,
    pub findings: Vec<Damage>,
}

impl VolumeHealth {
    /// The state the findings imply, worst first: [`HealthState::Damaged`]
    /// when any is an error, [`HealthState::NeedsRecovery`] when any is a
    /// notice, [`HealthState::Degraded`] when any is a warning, otherwise
    /// [`HealthState::Clean`]. So a clean volume carries no findings.
    pub fn from_findings(findings: Vec<Damage>) -> Self {
        let worst = |severity| findings.iter().any(|f| f.severity == severity);
        let state = if worst(Severity::Error) {
            HealthState::Damaged
        } else if worst(Severity::Notice) {
            HealthState::NeedsRecovery
        } else if worst(Severity::Warning) {
            HealthState::Degraded
        } else {
            HealthState::Clean
        };
        Self { state, findings }
    }

    pub fn is_clean(&self) -> bool {
        self.state == HealthState::Clean
    }
}

/// One copy of the boot record or superblock, decoded where it could be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperblockCopy<S> {
    /// 0 for the primary, 1 and up for backups, in the format's order.
    pub copy: u32,
    /// Where the copy is, or where it should be.
    pub location: Extent,
    /// The decoded copy, or `None` when its bytes could not be read or do
    /// not decode as the format.
    pub superblock: Option<S>,
    /// The format's own check over the copy, where it has one.
    pub integrity: Integrity,
}

/// What the driver supports on this volume: which layers and verbs can be
/// used, and which of the format's features it cannot read here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSupport {
    /// [`FileAPI`](super::FileAPI) can browse this volume: its namespace
    /// validated when it opened.
    pub files: bool,
    /// The salvage features the driver lacks, as its adapter reports them.
    pub salvage: Capabilities,
    /// Format features this volume uses that the driver cannot read, in the
    /// format's own names (an ext4 incompat flag, an NTFS attribute type).
    /// Reads that need one are refused, never guessed.
    pub unsupported: Vec<String>,
}

/// How sure a probe is that the source holds the format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Recognition {
    /// Nothing identifies the format.
    No,
    /// Some identifying bytes match and others are damaged or disagree: worth
    /// opening for salvage.
    Damaged,
    /// The format's identifying structures match.
    Yes,
}

/// What [`OpenVolume::probe`] found, without opening anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// The format name, as [`VolumeAPI::format`] gives it.
    pub format: &'static str,
    pub recognition: Recognition,
    /// The volume length the format records, when the probe could read it.
    pub declared_bytes: Option<u64>,
    /// What matched or disagreed, for a person reading it.
    pub detail: String,
}

/// The bottom layer: the volume as bytes, geometry, superblock and health.
///
/// Read-only by type: nothing here writes, and a driver's writable surface,
/// where it has one, is its own. Every method may be called on a volume
/// opened for salvage; one that cannot be answered from the surviving
/// structures fails rather than guessing.
pub trait VolumeAPI {
    /// The driver's error. It carries the shared refusals and device errors,
    /// so provided methods can raise them.
    type Error: std::error::Error + From<ApiError> + From<crate::Error> + 'static;
    /// The driver's decoded boot sector or superblock, as plain data.
    type Superblock: Clone + fmt::Debug;

    /// The format's name, lowercase: `"fat32"`, `"exfat"`, `"ntfs"`, `"ext4"`.
    fn format(&self) -> &'static str;

    /// Bytes this volume can read: the smaller of the source and, where the
    /// driver bounds it, the declared length.
    fn size_bytes(&self) -> u64;

    /// Reads volume-relative bytes. Fails with [`ApiError::Bounds`] when
    /// any of them lie outside [`Self::size_bytes`]; never reads short.
    fn read_volume_bytes(&self, offset: u64, output: &mut [u8]) -> Result<(), Self::Error>;

    /// The format-neutral geometry, decoded from the boot sector or
    /// superblock the volume was opened with.
    fn geometry(&self) -> Result<Geometry, Self::Error>;

    /// The decoded boot sector or superblock the volume was opened with.
    fn superblock(&self) -> Result<Self::Superblock, Self::Error>;

    /// Every copy of the boot record or superblock the format keeps, primary
    /// first, each decoded where it could be. A damaged copy is listed with
    /// its integrity failed, never left out.
    fn superblocks(&self) -> Result<Vec<SuperblockCopy<Self::Superblock>>, Self::Error>;

    /// What the driver supports on this volume.
    fn support(&self) -> Result<VolumeSupport, Self::Error>;

    /// The quick health verdict; see [`VolumeHealth`].
    fn health(&self) -> Result<VolumeHealth, Self::Error>;
}

/// Opening a volume from a source. Kept apart from [`VolumeAPI`] so that
/// trait stays usable as a trait object.
pub trait OpenVolume: VolumeAPI + Sized {
    /// The device the volume holds. It need only be readable.
    type Source: BlockRead;

    /// Says whether the source holds this format, reading only the bytes
    /// that identify it. An unreadable source is an error; an unrecognised
    /// one is [`Recognition::No`].
    fn probe(source: &dyn BlockRead) -> Result<Probe, crate::Error>;

    /// Opens a volume whose structures validate, ready for
    /// [`FileAPI`](super::FileAPI).
    fn open(source: Self::Source) -> Result<Self, Self::Error>;

    /// Opens with only what salvage needs -- usually a usable geometry, from
    /// a backup copy if the primary is damaged -- so damage below the
    /// geometry does not refuse the open. Defaults to [`Self::open`].
    fn open_for_salvage(source: Self::Source) -> Result<Self, Self::Error> {
        Self::open(source)
    }
}

/// Checks a volume-relative read against `size`: the shared bound every
/// [`VolumeAPI::read_volume_bytes`] applies.
pub fn check_bounds(offset: u64, length: usize, size: u64) -> Result<(), ApiError> {
    let length = length as u64;
    match offset.checked_add(length) {
        Some(end) if end <= size => Ok(()),
        _ => Err(ApiError::Bounds {
            offset,
            length,
            size,
        }),
    }
}
