//! PrimitiveAPI: read-only, damage-tolerant access to the on-disk structures,
//! and the salvage verbs bound to the volume's own source.
//!
//! Each format keeps its own primitives -- FAT entries, MFT records, inodes,
//! B-tree nodes -- as inherent methods or a driver trait beneath this one.
//! What is shared is the map of where the structures are
//! ([`PrimitiveAPI::structures`]) and the salvage verbs
//! ([`SalvageAdapter`]), which every format answers in the same terms.

use super::salvage::{
    Candidate, Capabilities, Damage, DamageReport, Extent, ExtentWalk, Integrity,
    MaterializeReport, Plan, PlanCheck, PlanReader, SalvageAdapter, SalvageError, ScanReport, Sink,
    Tree,
};
use super::salvage::{Encoding, Feature};
use super::volume::VolumeAPI;
use crate::BlockRead;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;

/// What a structure is for, in format-neutral terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum StructureRole {
    /// A boot sector or boot region.
    Boot,
    /// A superblock or volume header.
    Superblock,
    /// A map of which allocation units are in use or chained: a FAT, a
    /// bitmap, a free-space tree.
    AllocationMap,
    /// A table of object records: the MFT, an inode table, a group
    /// descriptor table.
    RecordTable,
    /// A directory, index or namespace tree root.
    Directory,
    /// A journal or log.
    Journal,
    /// A table that maps addresses: a chunk tree, an extent tree root.
    AddressMap,
    Other,
}

/// One on-disk structure and where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Structure {
    /// The structure's name in the format's own terms: `"boot sector"`,
    /// `"FAT 2"`, `"$MFTMirr"`, `"superblock backup in group 1"`.
    pub name: String,
    pub role: StructureRole,
    /// Its bytes on the source, or where they should be.
    pub location: Extent,
    /// 0 for the primary, 1 and up for backups and mirrors.
    pub copy: u32,
    /// The format's own check over the structure, where it has one.
    pub integrity: Integrity,
}

/// The middle layer: on-disk structures and the salvage verbs, read-only.
///
/// An implementation holds its source as the [`VolumeAPI`] beneath it does;
/// [`Self::source`] lends it out read-only, and [`Self::salvage`] binds the
/// format's [`SalvageAdapter`] to it so a caller does not pass the source
/// again. The adapter itself still takes `&dyn BlockRead` per call, so it can
/// be run over any other source, such as a recovered image.
pub trait PrimitiveAPI: VolumeAPI {
    /// The format's salvage adapter: format context only, never a device.
    type Adapter: SalvageAdapter;

    /// What a caller may suppose about the geometry when the structures
    /// that record it are gone: block size, metadata start, cluster size,
    /// in the format's own terms. Plain data, so a caller that does not know
    /// the format can still pass one back from [`Self::hypotheses`]. A
    /// driver that keeps a backup of every such structure, and so never
    /// needs one, uses `()`.
    type Hypothesis: Clone + fmt::Debug + Serialize + DeserializeOwned;

    /// The source the volume reads, as a read-only device.
    fn source(&self) -> &dyn BlockRead;

    /// The salvage adapter for this volume, built from whatever geometry
    /// survived. Fails with [`SalvageError::NoGeometry`] when none did.
    fn adapter(&self) -> Result<Self::Adapter, SalvageError>;

    /// Every structure the format places at a known location, primaries and
    /// copies, whether or not it validates. Damaged ones are listed with
    /// their integrity failed, never left out.
    fn structures(&self) -> Result<Vec<Structure>, SalvageError>;

    /// Plausible geometry hypotheses for this source, most likely first:
    /// what the driver can infer from the bytes that survive, such as the
    /// block sizes the format allows or a metadata start found by scanning.
    /// Empty by default.
    fn hypotheses(&self) -> Result<Vec<Self::Hypothesis>, SalvageError> {
        Ok(Vec::new())
    }

    /// The salvage adapter for this volume built from a caller's hypothesis
    /// rather than from the surviving structures, for a source whose
    /// superblock is gone. The results are evidence under that hypothesis.
    /// Defaults to `Unsupported(GeometryHypotheses)`.
    fn adapter_with(&self, hypothesis: &Self::Hypothesis) -> Result<Self::Adapter, SalvageError> {
        let _ = hypothesis;
        Err(SalvageError::Unsupported(Feature::GeometryHypotheses))
    }

    /// The salvage verbs, bound to this volume's source.
    fn salvage(&self) -> Result<Salvage<'_, Self::Adapter>, SalvageError> {
        Ok(Salvage {
            adapter: self.adapter()?,
            source: self.source(),
        })
    }

    /// The salvage verbs under a geometry hypothesis, bound to this volume's
    /// source.
    fn salvage_with(
        &self,
        hypothesis: &Self::Hypothesis,
    ) -> Result<Salvage<'_, Self::Adapter>, SalvageError> {
        Ok(Salvage {
            adapter: self.adapter_with(hypothesis)?,
            source: self.source(),
        })
    }
}

/// A [`SalvageAdapter`] bound to a source. Each method is the adapter's,
/// with the source filled in.
pub struct Salvage<'a, A> {
    adapter: A,
    source: &'a dyn BlockRead,
}

impl<'a, A: SalvageAdapter> Salvage<'a, A> {
    /// Binds `adapter` to `source`, for an adapter used without a volume.
    pub fn new(adapter: A, source: &'a dyn BlockRead) -> Self {
        Self { adapter, source }
    }

    pub fn adapter(&self) -> &A {
        &self.adapter
    }

    pub fn source(&self) -> &'a dyn BlockRead {
        self.source
    }

    pub fn capabilities(&self) -> Capabilities {
        self.adapter.capabilities()
    }

    pub fn find_candidates(
        &self,
        range: Extent,
        sink: &mut dyn Sink<Candidate<A::Detail>>,
    ) -> Result<ScanReport, SalvageError> {
        self.adapter.find_candidates(self.source, range, sink)
    }

    /// [`Self::find_candidates`] over the whole source, collected.
    pub fn all_candidates(&self) -> Result<Vec<Candidate<A::Detail>>, SalvageError> {
        let mut candidates = Vec::new();
        self.find_candidates(Extent::new(0, self.source.size_bytes()), &mut candidates)?;
        Ok(candidates)
    }

    pub fn walk_extents(
        &self,
        candidate: &Candidate<A::Detail>,
    ) -> Result<ExtentWalk, SalvageError> {
        self.adapter.walk_extents(self.source, candidate)
    }

    pub fn rebuild_tree(&self, candidates: &[Candidate<A::Detail>]) -> Result<Tree, SalvageError> {
        self.adapter.rebuild_tree(self.source, candidates)
    }

    pub fn report_damage(&self, sink: &mut dyn Sink<Damage>) -> Result<DamageReport, SalvageError> {
        self.adapter.report_damage(self.source, sink)
    }

    pub fn read_encoded(
        &self,
        stored: Extent,
        encoding: &Encoding,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        self.adapter
            .read_encoded(self.source, stored, encoding, offset, output)
    }

    pub fn read_extent(
        &self,
        extent: Extent,
        offset: u64,
        output: &mut [u8],
    ) -> Result<usize, SalvageError> {
        self.adapter
            .read_extent(self.source, extent, offset, output)
    }

    pub fn materialize(
        &self,
        plan: &Plan,
        sink: &mut dyn io::Write,
    ) -> Result<MaterializeReport, SalvageError> {
        self.adapter.materialize(self.source, plan, sink)
    }

    pub fn plan_reader<'p>(&'p self, plan: &'p Plan) -> Result<PlanReader<'p, A>, SalvageError>
    where
        A: Sized,
    {
        self.adapter.plan_reader(self.source, plan)
    }

    pub fn check_plan(&self, plan: &Plan) -> Result<PlanCheck, SalvageError> {
        self.adapter.check_plan(plan, self.source.size_bytes())
    }
}
