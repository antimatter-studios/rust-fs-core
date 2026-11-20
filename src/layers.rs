//! The three API layers every rust-fs driver implements, behind the
//! `layers` feature.
//!
//! | Layer | Trait | What it does |
//! |---|---|---|
//! | Volume | [`VolumeAPI`], [`OpenVolume`] | Open, bytes, geometry, boot sector or superblock and its copies, health, supported features |
//! | Primitive | [`PrimitiveAPI`], [`SalvageAdapter`] | Read-only, damage-tolerant structures and the salvage verbs |
//! | File | [`FileAPI`] | Browse, read and copy on a healthy volume |
//!
//! Each layer builds on the one below: [`PrimitiveAPI`] and [`FileAPI`] both
//! require [`VolumeAPI`], and neither requires the other, so a damaged
//! volume opened for salvage can offer the primitives without a namespace.
//! Nothing in any layer writes; a driver's writable surface stays its own.
//!
//! Only what is generic across formats lives here. Each format's own
//! primitives -- FAT chains, exFAT entry sets, NTFS records, ext4 inodes and
//! extent trees, XFS AG B-trees, Btrfs and bcachefs tree nodes, EROFS
//! inodes, SquashFS metablocks -- stay in its driver, behind that driver's
//! [`PrimitiveAPI`] implementation.
//!
//! [`conformance`] checks a driver's implementation against the contracts
//! written here, over the driver's own test images. `docs/three-layers.md`
//! is the narrative, and says how drivers pin this crate.

pub mod conformance;
pub mod file;
pub mod primitive;
pub mod salvage;
pub mod volume;

pub use file::{DirEntry, FileAPI, Metadata, NodeKind, Timestamp};
pub use primitive::{PrimitiveAPI, Salvage, Structure, StructureRole};
pub use salvage::{
    Candidate, CandidateKind, CandidateState, Capabilities, ClusterLayout, Damage, DamageKind,
    DamageReport, Encoding, Extent, ExtentWalk, Feature, Integrity, MaterializeReport,
    MaterializedRange, Parent, ParentClaim, Plan, PlanCheck, PlanFault, PlanMetadata, PlanOverlap,
    PlanRead, PlanReader, PlanStep, SalvageAdapter, SalvageError, ScanReport, Severity, Sink, Tree,
    TreeNode, WalkEnd, WalkExtent,
};
pub use volume::{
    check_bounds, ApiError, Geometry, HealthState, OpenVolume, Probe, Recognition, SuperblockCopy,
    VolumeAPI, VolumeHealth, VolumeSupport,
};
