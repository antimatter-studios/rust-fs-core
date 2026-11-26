//! A conformance check any driver runs against its own images.
//!
//! Each function takes an opened volume, exercises one layer's contract and
//! returns a [`Report`] of named checks. A driver's test opens each of its
//! images and asserts the report passed:
//!
//! ```no_run
//! use fs_core::layers::conformance::{self, Expected, Limits};
//! use fs_core::layers::{FileAPI, PrimitiveAPI};
//!
//! fn image_conforms<V: FileAPI + PrimitiveAPI>(volume: &V) {
//!     conformance::check_healthy(
//!         volume,
//!         &[Expected::file("/hello.txt", b"hello\n"), Expected::directory("/docs")],
//!         &Limits::default(),
//!     )
//!     .assert_passed();
//! }
//! ```
//!
//! # What it proves, and what it does not
//!
//! The checks hold a driver to the shape of the contract: bounds are
//! refused, flags and refusals agree, streamed counts match, the layers agree
//! with each other about the same bytes, and every reachable file reads back
//! its declared length. They cannot tell whether the driver reads the format
//! correctly: a misreading baked into both the image and the parser passes.
//! That still needs an independent oracle, per the family's rules.
//!
//! [`salvage`] is the deeper set for the salvage verbs alone, over images
//! the driver builds with declared damage: [`salvage::run`] with a
//! [`salvage::Driver`], or, for images too large to hold in memory,
//! [`salvage::run_sources`] with a [`salvage::SourceDriver`].
//!
//! Nothing skips. A verb a driver lacks passes only when its capabilities
//! say so and the verb refuses with `Unsupported` naming it. An empty report
//! fails, and a tree larger than the [`Limits`] fails rather than being
//! checked in part.

pub mod salvage;

use super::file::{DirEntry, FileAPI, Metadata, NodeKind};
use super::primitive::{PrimitiveAPI, StructureRole};
use super::salvage::{
    Candidate, CandidateKind, CandidateState, Capabilities, Extent, Feature, Plan, PlanStep,
    SalvageAdapter, SalvageError, WalkEnd,
};
use super::volume::{HealthState, VolumeAPI};
use std::collections::HashSet;
use std::fmt;
use std::ops::ControlFlow;

/// One named check and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    /// What was observed: on a failure, enough to find the cause.
    pub detail: String,
}

/// The outcome of a conformance run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// Records a check.
    pub fn record(&mut self, name: &str, passed: bool, detail: impl Into<String>) {
        self.checks.push(Check {
            name: name.to_owned(),
            passed,
            detail: detail.into(),
        });
    }

    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|check| !check.passed)
    }

    /// True when at least one check ran and none failed.
    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.failures().next().is_none()
    }

    /// Appends another report's checks.
    pub fn merge(&mut self, other: Report) {
        self.checks.extend(other.checks);
    }

    /// Panics, listing every failure, unless [`Self::passed`].
    #[track_caller]
    pub fn assert_passed(&self) {
        assert!(self.passed(), "conformance failed:\n{self}");
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.checks.is_empty() {
            return formatter.write_str("no checks ran\n");
        }
        for check in &self.checks {
            let verdict = if check.passed { "pass" } else { "FAIL" };
            writeln!(formatter, "{verdict} {}: {}", check.name, check.detail)?;
        }
        Ok(())
    }
}

/// How much of an image the checks walk. A tree or scan larger than these
/// fails rather than being checked in part; raise them for a larger image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Directory entries visited through [`FileAPI`].
    pub max_nodes: usize,
    /// Directory depth below the root.
    pub max_depth: usize,
    /// Candidates whose extents are walked.
    pub max_walks: usize,
    /// Paths resolved through [`FileAPI::lookup_bytes`], spread evenly over
    /// the tree: each lookup reads every directory on its path, so one per
    /// entry is quadratic in a large directory. Lossy names are always
    /// resolved.
    pub max_lookups: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_nodes: 4096,
            max_depth: 32,
            max_walks: 1024,
            max_lookups: 4096,
        }
    }
}

/// What a test image is known to hold, checked through [`FileAPI`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expected {
    File { path: String, contents: Vec<u8> },
    Directory { path: String },
    Absent { path: String },
}

impl Expected {
    pub fn file(path: &str, contents: &[u8]) -> Self {
        Self::File {
            path: path.to_owned(),
            contents: contents.to_vec(),
        }
    }

    pub fn directory(path: &str) -> Self {
        Self::Directory {
            path: path.to_owned(),
        }
    }

    pub fn absent(path: &str) -> Self {
        Self::Absent {
            path: path.to_owned(),
        }
    }
}

/// Every check, for a healthy image: the volume reports itself clean, the
/// primitives and salvage verbs keep their contracts, and the namespace holds
/// what `expected` says and reads back consistently.
pub fn check_healthy<V: FileAPI + PrimitiveAPI>(
    volume: &V,
    expected: &[Expected],
    limits: &Limits,
) -> Report {
    let mut report = check_volume(volume, true);
    report.merge(check_primitive(volume, limits));
    report.merge(check_files(volume, expected, limits));
    report
}

/// The checks that hold on a damaged image opened for salvage: the volume
/// and primitive layers, with no claim that the volume is clean.
pub fn check_damaged<V: PrimitiveAPI>(volume: &V, limits: &Limits) -> Report {
    let mut report = check_volume(volume, false);
    report.merge(check_primitive(volume, limits));
    report
}

/// [`VolumeAPI`]: format name, bounded reads, geometry, superblock and
/// health. With `expect_clean`, the health verdict must be clean.
pub fn check_volume<V: VolumeAPI + ?Sized>(volume: &V, expect_clean: bool) -> Report {
    let mut report = Report::default();
    let format = volume.format();
    report.record(
        "volume.format",
        !format.is_empty()
            && format
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        format!("{format:?}"),
    );
    let size = volume.size_bytes();
    report.record("volume.size", size > 0, format!("{size} bytes"));
    if size > 0 {
        let mut head = vec![0u8; size.min(512) as usize];
        let first = volume.read_volume_bytes(0, &mut head);
        report.record("volume.read-first", first.is_ok(), describe(&first));
        let mut last = [0u8; 1];
        let tail = volume.read_volume_bytes(size - 1, &mut last);
        report.record("volume.read-last", tail.is_ok(), describe(&tail));
        let mut two = [0u8; 2];
        let straddle = volume.read_volume_bytes(size - 1, &mut two);
        report.record(
            "volume.read-straddling-end-refused",
            straddle.is_err(),
            describe(&straddle),
        );
    }
    let mut one = [0u8; 1];
    let past = volume.read_volume_bytes(size, &mut one);
    report.record(
        "volume.read-past-end-refused",
        past.is_err(),
        describe(&past),
    );
    let wrap = volume.read_volume_bytes(u64::MAX, &mut one);
    report.record(
        "volume.read-at-u64-max-refused",
        wrap.is_err(),
        describe(&wrap),
    );

    match volume.geometry() {
        Ok(geometry) => {
            let units = geometry.sector_size > 0
                && geometry.sector_size.is_power_of_two()
                && geometry.allocation_unit > 0
                && geometry.allocation_unit % u64::from(geometry.sector_size) == 0;
            report.record(
                "volume.geometry-units",
                units,
                format!(
                    "sector {} bytes, allocation unit {} bytes",
                    geometry.sector_size, geometry.allocation_unit
                ),
            );
            if let Some(layout) = geometry.layout {
                report.record(
                    "volume.geometry-layout",
                    layout.size == geometry.allocation_unit
                        && layout.count <= geometry.allocation_units
                        && layout.offset <= geometry.declared_bytes,
                    format!("{layout:?} within {geometry:?}"),
                );
            }
        }
        Err(error) => report.record("volume.geometry", false, error.to_string()),
    }
    let superblock = volume.superblock();
    report.record(
        "volume.superblock",
        superblock.is_ok(),
        match &superblock {
            Ok(_) => "decoded".to_owned(),
            Err(error) => error.to_string(),
        },
    );
    match volume.superblocks() {
        Ok(copies) => {
            let primary_first = copies.first().is_some_and(|c| c.copy == 0)
                && copies.iter().filter(|c| c.copy == 0).count() == 1;
            let representable = copies.iter().all(|c| c.location.end().is_some());
            report.record(
                "volume.superblock-copies",
                primary_first && representable,
                format!(
                    "{:?}",
                    copies
                        .iter()
                        .map(|c| (c.copy, c.location, c.integrity, c.superblock.is_some()))
                        .collect::<Vec<_>>()
                ),
            );
            if expect_clean {
                report.record(
                    "volume.superblock-primary-valid",
                    copies.first().is_some_and(|c| {
                        c.superblock.is_some() && c.integrity != super::salvage::Integrity::Failed
                    }),
                    format!("{:?}", copies.first().map(|c| (c.location, c.integrity))),
                );
            }
        }
        Err(error) => report.record("volume.superblock-copies", false, error.to_string()),
    }
    match volume.support() {
        Ok(support) => {
            report.record("volume.support", true, format!("{support:?}"));
            if expect_clean {
                report.record(
                    "volume.support-files-on-a-healthy-volume",
                    support.files,
                    format!("{support:?}"),
                );
            }
        }
        Err(error) => report.record("volume.support", false, error.to_string()),
    }
    match volume.health() {
        Ok(health) => {
            report.record(
                "volume.health-consistent",
                // The state is what the findings imply, unless the driver
                // cannot tell.
                health.state == HealthState::Unknown
                    || health.state
                        == super::volume::VolumeHealth::from_findings(health.findings.clone())
                            .state,
                format!(
                    "{:?} with {} finding(s)",
                    health.state,
                    health.findings.len()
                ),
            );
            if expect_clean {
                report.record(
                    "volume.health-clean",
                    health.is_clean(),
                    format!("{:?}: {:?}", health.state, health.findings),
                );
            }
        }
        Err(error) => report.record("volume.health", false, error.to_string()),
    }
    report
}

/// [`PrimitiveAPI`] and the salvage verbs through it.
pub fn check_primitive<V: PrimitiveAPI>(volume: &V, limits: &Limits) -> Report {
    let mut report = Report::default();
    let salvage = match volume.salvage() {
        Ok(salvage) => salvage,
        Err(error) => {
            report.record("primitive.adapter", false, error.to_string());
            return report;
        }
    };
    let adapter = salvage.adapter();
    let source = volume.source();
    let size = source.size_bytes();
    report.record(
        "primitive.source-size",
        size == volume.size_bytes(),
        format!("source {size}, volume {}", volume.size_bytes()),
    );
    let capabilities = adapter.capabilities();
    match volume.support() {
        Ok(support) => report.record(
            "primitive.capabilities-match-volume-support",
            support.salvage == capabilities,
            format!(
                "adapter missing {:?}, volume says {:?}",
                capabilities.missing, support.salvage.missing
            ),
        ),
        Err(error) => report.record("primitive.capabilities", false, error.to_string()),
    }

    check_hypotheses(volume, &mut report);
    check_structures(volume, &salvage, &capabilities, &mut report);
    let candidates = check_scan(&salvage, &capabilities, size, &mut report);
    if let Some(candidates) = &candidates {
        check_walks(
            &salvage,
            &capabilities,
            candidates,
            size,
            limits,
            &mut report,
        );
        check_tree(&salvage, &capabilities, candidates, &mut report);
    }
    check_damage(&salvage, &capabilities, size, &mut report);
    check_read_extent(&salvage, &capabilities, size, &mut report);
    check_materialize(volume, &salvage, &capabilities, size, &mut report);
    report
}

/// Every hypothesis the driver offers builds an adapter.
fn check_hypotheses<V: PrimitiveAPI>(volume: &V, report: &mut Report) {
    let hypotheses = match volume.hypotheses() {
        Ok(hypotheses) => hypotheses,
        Err(error) => return report.record("primitive.hypotheses", false, error.to_string()),
    };
    let mut problems = Vec::new();
    for hypothesis in hypotheses.iter().take(8) {
        if let Err(error) = volume.adapter_with(hypothesis) {
            problems.push(format!("{hypothesis:?}: {error:?}"));
        }
    }
    report.record(
        "primitive.hypotheses-build-adapters",
        problems.is_empty(),
        format!("{} hypothesis(es); {problems:?}", hypotheses.len()),
    );
}

type Bound<'a, V> = super::primitive::Salvage<'a, <V as PrimitiveAPI>::Adapter>;

fn check_structures<V: PrimitiveAPI>(
    volume: &V,
    salvage: &Bound<'_, V>,
    capabilities: &Capabilities,
    report: &mut Report,
) {
    let structures = match volume.structures() {
        Ok(structures) => structures,
        Err(error) => {
            report.record("primitive.structures", false, error.to_string());
            return;
        }
    };
    report.record(
        "primitive.structures-listed",
        !structures.is_empty(),
        format!("{} structure(s)", structures.len()),
    );
    report.record(
        "primitive.structures-include-boot-or-superblock",
        structures
            .iter()
            .any(|s| matches!(s.role, StructureRole::Boot | StructureRole::Superblock)),
        format!(
            "{:?}",
            structures.iter().map(|s| &s.name).collect::<Vec<_>>()
        ),
    );
    let unrepresentable: Vec<_> = structures
        .iter()
        .filter(|s| s.location.end().is_none())
        .map(|s| s.name.clone())
        .collect();
    report.record(
        "primitive.structures-representable",
        unrepresentable.is_empty(),
        format!("unrepresentable: {unrepresentable:?}"),
    );
    // The two layers must agree about the same bytes.
    let size = volume.size_bytes();
    let mut disagreements = Vec::new();
    for structure in structures
        .iter()
        .filter(|s| s.location.end().is_some_and(|end| end <= size) && s.location.length > 0)
        .take(8)
    {
        let length = structure.location.length.min(1 << 20) as usize;
        let mut through_volume = vec![0u8; length];
        let mut through_extent = vec![0u8; length];
        let volume_read = volume.read_volume_bytes(structure.location.offset, &mut through_volume);
        let extent_read = salvage.read_extent(structure.location, 0, &mut through_extent);
        match (volume_read, extent_read) {
            (Ok(()), Ok(read)) if read == length && through_volume == through_extent => {}
            (_, Err(SalvageError::Unsupported(Feature::ReadExtent)))
                if !capabilities.supports(Feature::ReadExtent) => {}
            (volume_read, extent_read) => disagreements.push(format!(
                "{}: volume {:?}, extent {:?}",
                structure.name,
                volume_read.map_err(|e| e.to_string()),
                extent_read.map_err(|e| e.to_string())
            )),
        }
    }
    report.record(
        "primitive.structures-read-alike",
        disagreements.is_empty(),
        format!("{disagreements:?}"),
    );
}

/// Whether a verb's result agrees with the capability flags: a missing
/// feature must refuse with `Unsupported` naming it, and a present one must
/// not.
fn flags_agree<T>(
    capabilities: &Capabilities,
    feature: Feature,
    result: &Result<T, SalvageError>,
) -> Result<bool, String> {
    let refused = matches!(result, Err(SalvageError::Unsupported(f)) if *f == feature);
    match (capabilities.supports(feature), refused) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (true, true) => Err(format!("{feature} is supported but refused")),
        (false, false) => Err(format!("{feature} is listed missing but did not refuse")),
    }
}

fn check_scan<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    capabilities: &Capabilities,
    size: u64,
    report: &mut Report,
) -> Option<Vec<Candidate<A::Detail>>> {
    let mut candidates = Vec::new();
    let scan = salvage.find_candidates(Extent::new(0, size), &mut candidates);
    match flags_agree(capabilities, Feature::FindCandidates, &scan) {
        Err(why) => {
            report.record("primitive.scan-flags", false, why);
            return None;
        }
        Ok(false) => {
            report.record("primitive.scan-refused-as-flagged", true, "find-candidates");
            return None;
        }
        Ok(true) => {}
    }
    let scan = match scan {
        Ok(scan) => scan,
        Err(error) => {
            report.record("primitive.scan", false, error.to_string());
            return None;
        }
    };
    report.record(
        "primitive.scan-count",
        scan.candidates == candidates.len() as u64 && scan.stopped_at.is_none(),
        format!(
            "report {} / sink {}, stopped {:?}",
            scan.candidates,
            candidates.len(),
            scan.stopped_at
        ),
    );
    let outside: Vec<_> = candidates
        .iter()
        .filter(|c| c.location.end().is_none_or(|end| end > size))
        .map(|c| c.location)
        .chain(
            scan.unreadable
                .iter()
                .filter(|e| e.end().is_none_or(|end| end > size))
                .copied(),
        )
        .collect();
    report.record(
        "primitive.scan-within-source",
        outside.is_empty(),
        format!("outside: {outside:?}"),
    );
    let past = salvage.find_candidates(Extent::new(size, 1), &mut Vec::new());
    report.record(
        "primitive.scan-past-end-refused",
        matches!(past, Err(SalvageError::Bounds)),
        describe_salvage(&past),
    );
    if candidates.len() >= 2 {
        check_resume(salvage, &candidates, size, report);
    }
    Some(candidates)
}

/// A scan stopped by its sink says where, and a scan resumed there loses
/// nothing.
fn check_resume<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    candidates: &[Candidate<A::Detail>],
    size: u64,
    report: &mut Report,
) {
    let adapter = salvage.adapter();
    let mut first = None;
    let mut stop = |candidate: Candidate<A::Detail>| {
        first = Some(adapter.candidate_id(&candidate));
        ControlFlow::Break(())
    };
    let stopped = salvage.find_candidates(Extent::new(0, size), &mut stop);
    let at = match &stopped {
        Ok(scan) => scan.stopped_at,
        Err(_) => None,
    };
    report.record(
        "primitive.scan-stop-says-where",
        at.is_some_and(|at| at <= size),
        format!("{:?}", stopped.as_ref().map(|scan| scan.stopped_at)),
    );
    let (Some(at), Some(first)) = (at, first) else {
        return;
    };
    let mut rest = Vec::new();
    let resumed = salvage.find_candidates(Extent::new(at, size - at), &mut rest);
    let found: HashSet<u64> = rest.iter().map(|c| adapter.candidate_id(c)).collect();
    let lost: Vec<u64> = candidates
        .iter()
        .map(|c| adapter.candidate_id(c))
        .filter(|id| *id != first && !found.contains(id))
        .collect();
    report.record(
        "primitive.scan-resume-loses-nothing",
        resumed.is_ok() && lost.is_empty(),
        format!(
            "resumed at {at}: lost {lost:?}, {}",
            describe_salvage(&resumed)
        ),
    );
}

fn check_walks<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    capabilities: &Capabilities,
    candidates: &[Candidate<A::Detail>],
    size: u64,
    limits: &Limits,
    report: &mut Report,
) {
    report.record(
        "primitive.walk-limit",
        candidates.len() <= limits.max_walks,
        format!(
            "{} candidate(s), limit {}",
            candidates.len(),
            limits.max_walks
        ),
    );
    let mut problems = Vec::new();
    let mut walked = 0usize;
    let mut decoded_extents = 0usize;
    for candidate in candidates.iter().take(limits.max_walks) {
        let walk = salvage.walk_extents(candidate);
        let deleted = candidate.state == CandidateState::Deleted;
        let refusal = match &walk {
            Err(SalvageError::Unsupported(feature)) => Some(*feature),
            _ => None,
        };
        let expected_refusal = if !capabilities.supports(Feature::WalkExtents) {
            Some(Feature::WalkExtents)
        } else if deleted && !capabilities.supports(Feature::DeletedExtents) {
            Some(Feature::DeletedExtents)
        } else {
            None
        };
        if refusal != expected_refusal {
            problems.push(format!(
                "{:?}: refused {refusal:?}, expected {expected_refusal:?}",
                candidate.location
            ));
            continue;
        }
        let Ok(walk) = walk else {
            if refusal.is_none() {
                problems.push(format!(
                    "{:?}: {}",
                    candidate.location,
                    describe_salvage(&walk)
                ));
            }
            continue;
        };
        walked += 1;
        let mapped = walk.mapped();
        let inside = walk
            .sources()
            .all(|e| e.end().is_some_and(|end| end <= size));
        let ordered = walk
            .extents
            .windows(2)
            .all(|pair| pair[0].end().is_some_and(|end| end <= pair[1].offset));
        let within_claim = walk
            .extents
            .iter()
            .all(|e| e.end().is_some_and(|end| end <= walk.claimed));
        for extent in walk.extents.iter().filter(|e| e.encoded.is_some()) {
            let encoding = extent.encoded.as_ref().expect("filtered");
            if !capabilities.supports(Feature::EncodedExtents) {
                problems.push(format!(
                    "{:?}: an encoded extent, though encoded-extents is listed missing",
                    candidate.location
                ));
                continue;
            }
            let want = encoding.length.min(64 * 1024) as usize;
            let mut decoded = vec![0u8; want];
            let read = salvage.read_encoded(extent.source, encoding, 0, &mut decoded);
            let at_end =
                salvage.read_encoded(extent.source, encoding, encoding.length, &mut [0u8; 1]);
            let shape = encoding
                .offset
                .checked_add(encoding.length)
                .is_some_and(|end| end <= encoding.decoded);
            if !shape {
                problems.push(format!(
                    "{:?}: run {}..+{} lies outside the {} decoded bytes",
                    candidate.location, encoding.offset, encoding.length, encoding.decoded
                ));
            }
            if !matches!(read, Ok(n) if n == want) || !matches!(at_end, Ok(0)) {
                problems.push(format!(
                    "{:?}: {} extent {:?} decodes {} and {} at its end",
                    candidate.location,
                    encoding.codec,
                    extent.source,
                    describe_salvage(&read),
                    describe_salvage(&at_end)
                ));
            } else {
                decoded_extents += 1;
            }
        }
        let plan_length = walk.plan().length();
        let complete_covers = walk.end != WalkEnd::Complete || plan_length == Some(walk.claimed);
        // File bytes are compared with the claim, and stored bytes only with
        // the source: an encoded unit can store more than it decodes to
        // (rust-fs-core#462).
        let covered = walk
            .extents
            .iter()
            .try_fold(0u64, |total, e| total.checked_add(e.length()));
        let fits = covered.is_some_and(|covered| covered <= walk.claimed);
        if !inside || !ordered || !within_claim || !fits || !complete_covers {
            problems.push(format!(
                "{:?}: covers {covered:?} file and maps {mapped} stored of {} claimed byte(s), end {:?}, inside {inside}, in file order {ordered}, within the claim {within_claim}, plan {plan_length:?}",
                candidate.location, walk.claimed, walk.end
            ));
        }
    }
    report.record(
        "primitive.walks",
        problems.is_empty(),
        format!("{walked} walked, {decoded_extents} encoded extent(s) decoded; {problems:?}"),
    );
    if !capabilities.supports(Feature::EncodedExtents) {
        let probe = salvage.read_encoded(
            Extent::new(0, 0),
            &super::salvage::Encoding::whole("none", 0),
            0,
            &mut [0u8; 1],
        );
        report.record(
            "primitive.encoded-refused-as-flagged",
            matches!(
                probe,
                Err(SalvageError::Unsupported(Feature::EncodedExtents))
            ),
            describe_salvage(&probe),
        );
    }
}

fn check_tree<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    capabilities: &Capabilities,
    candidates: &[Candidate<A::Detail>],
    report: &mut Report,
) {
    let tree = salvage.rebuild_tree(candidates);
    match flags_agree(capabilities, Feature::RebuildTree, &tree) {
        Err(why) => return report.record("primitive.tree-flags", false, why),
        Ok(false) => return report.record("primitive.tree-refused-as-flagged", true, ""),
        Ok(true) => {}
    }
    let tree = match tree {
        Ok(tree) => tree,
        Err(error) => return report.record("primitive.tree", false, error.to_string()),
    };
    let adapter = salvage.adapter();
    let offsets: HashSet<u64> = candidates.iter().map(|c| adapter.candidate_id(c)).collect();
    let in_order = tree.nodes.len() == candidates.len()
        && tree
            .nodes
            .iter()
            .zip(candidates)
            .all(|(node, candidate)| node.candidate == adapter.candidate_id(candidate));
    let dangling: Vec<_> = tree
        .nodes
        .iter()
        .flat_map(|node| node.parents.iter())
        .filter_map(|claim| match claim.parent {
            super::salvage::Parent::Candidate(offset) if !offsets.contains(&offset) => Some(offset),
            _ => None,
        })
        .collect();
    report.record(
        "primitive.tree-one-node-per-candidate",
        in_order,
        format!(
            "{} node(s) for {} candidate(s)",
            tree.nodes.len(),
            candidates.len()
        ),
    );
    report.record(
        "primitive.tree-parents-are-candidates",
        dangling.is_empty(),
        format!("parents outside the input: {dangling:?}"),
    );
}

fn check_damage<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    capabilities: &Capabilities,
    _size: u64,
    report: &mut Report,
) {
    let mut findings = Vec::new();
    let damage = salvage.report_damage(&mut findings);
    match flags_agree(capabilities, Feature::ReportDamage, &damage) {
        Err(why) => return report.record("primitive.damage-flags", false, why),
        Ok(false) => return report.record("primitive.damage-refused-as-flagged", true, ""),
        Ok(true) => {}
    }
    match damage {
        Ok(damage) => report.record(
            "primitive.damage-count",
            damage.reported == findings.len() as u64
                && !damage.stopped
                && findings.iter().all(|f| f.location.end().is_some()),
            format!("report {} / sink {}", damage.reported, findings.len()),
        ),
        Err(error) => report.record("primitive.damage", false, error.to_string()),
    }
}

fn check_read_extent<A: SalvageAdapter>(
    salvage: &super::primitive::Salvage<'_, A>,
    capabilities: &Capabilities,
    size: u64,
    report: &mut Report,
) {
    let mut buffer = [0u8; 16];
    let length = size.min(16);
    let read = salvage.read_extent(Extent::new(0, length), 0, &mut buffer);
    match flags_agree(capabilities, Feature::ReadExtent, &read) {
        Err(why) => return report.record("primitive.read-extent-flags", false, why),
        Ok(false) => return report.record("primitive.read-extent-refused-as-flagged", true, ""),
        Ok(true) => {}
    }
    report.record(
        "primitive.read-extent",
        matches!(read, Ok(n) if n as u64 == length),
        describe_salvage(&read),
    );
    let at_end = salvage.read_extent(Extent::new(0, length), length, &mut buffer);
    report.record(
        "primitive.read-extent-at-its-end-reads-nothing",
        matches!(at_end, Ok(0)),
        describe_salvage(&at_end),
    );
    let past_offset = salvage.read_extent(Extent::new(0, length), length + 1, &mut buffer);
    report.record(
        "primitive.read-extent-offset-past-extent-refused",
        matches!(past_offset, Err(SalvageError::Bounds)),
        describe_salvage(&past_offset),
    );
    let past_source = salvage.read_extent(Extent::new(size.saturating_sub(1), 2), 0, &mut buffer);
    report.record(
        "primitive.read-extent-past-source-refused",
        matches!(past_source, Err(SalvageError::Bounds)),
        describe_salvage(&past_source),
    );
}

fn check_materialize<V: PrimitiveAPI>(
    volume: &V,
    salvage: &Bound<'_, V>,
    capabilities: &Capabilities,
    size: u64,
    report: &mut Report,
) {
    let length = size.min(4096);
    let literal = [0xA5u8, 0x5A];
    let plan = Plan::new(vec![
        PlanStep::Extent(Extent::new(0, length)),
        PlanStep::Zeros(3),
        PlanStep::Bytes(literal.to_vec()),
    ]);
    let mut output = Vec::new();
    let materialized = salvage.materialize(&plan, &mut output);
    match flags_agree(capabilities, Feature::Materialize, &materialized) {
        Err(why) => return report.record("primitive.materialize-flags", false, why),
        Ok(false) => return report.record("primitive.materialize-refused-as-flagged", true, ""),
        Ok(true) => {}
    }
    let mut expected = vec![0u8; length as usize];
    let source_read = volume.read_volume_bytes(0, &mut expected);
    expected.extend_from_slice(&[0, 0, 0]);
    expected.extend_from_slice(&literal);
    match materialized {
        Ok(materialized) => report.record(
            "primitive.materialize",
            source_read.is_ok()
                && output == expected
                && materialized.length == expected.len() as u64
                && materialized.source_of(0) == (length > 0).then_some(0)
                && materialized.unreadable().next().is_none(),
            format!(
                "{} byte(s), {} range(s)",
                output.len(),
                materialized.ranges.len()
            ),
        ),
        Err(error) => report.record("primitive.materialize", false, error.to_string()),
    }
    let past = salvage.check_plan(&Plan::new(vec![PlanStep::Extent(Extent::new(size, 1))]));
    report.record(
        "primitive.plan-past-source-refused",
        matches!(past, Err(SalvageError::Bounds)),
        describe_salvage(&past),
    );

    let layout = salvage.adapter().cluster_layout();
    match flags_agree(capabilities, Feature::ClusterSteps, &layout) {
        Err(why) => return report.record("primitive.cluster-flags", false, why),
        Ok(false) => return report.record("primitive.cluster-refused-as-flagged", true, ""),
        Ok(true) => {}
    }
    let layout = match layout {
        Ok(layout) => layout,
        Err(error) => return report.record("primitive.cluster-layout", false, error.to_string()),
    };
    if let Ok(geometry) = volume.geometry() {
        report.record(
            "primitive.cluster-layout-matches-geometry",
            geometry.layout.is_none_or(|g| g == layout),
            format!("adapter {layout:?}, geometry {:?}", geometry.layout),
        );
    }
    let Some(first) = layout
        .extent(layout.first, 1)
        .filter(|e| e.end().is_some_and(|end| end <= size))
    else {
        return report.record(
            "primitive.cluster-layout-in-source",
            false,
            format!("first cluster of {layout:?} is outside the {size}-byte source"),
        );
    };
    let mut through_plan = Vec::new();
    let plan = Plan::new(vec![PlanStep::Clusters {
        first: layout.first,
        count: 1,
    }]);
    let ran = salvage.materialize(&plan, &mut through_plan);
    let mut through_volume = vec![0u8; first.length as usize];
    let read = volume.read_volume_bytes(first.offset, &mut through_volume);
    report.record(
        "primitive.cluster-step-reads-the-cluster",
        ran.is_ok() && read.is_ok() && through_plan == through_volume,
        format!("cluster {} at {:?}", layout.first, first),
    );
}

/// [`FileAPI`]: the whole tree walks, every file reads back its declared
/// length the same way twice, paths resolve to the nodes listed, and the
/// image holds what `expected` says.
pub fn check_files<V: FileAPI>(volume: &V, expected: &[Expected], limits: &Limits) -> Report {
    let mut report = Report::default();
    let root = match volume.root() {
        Ok(root) => root,
        Err(error) => {
            report.record("file.root", false, error.to_string());
            return report;
        }
    };
    let root_metadata = volume.metadata(&root);
    report.record(
        "file.root-is-directory",
        matches!(&root_metadata, Ok(m) if m.kind == NodeKind::Directory),
        format!(
            "{:?}",
            root_metadata
                .as_ref()
                .map(|m| m.kind)
                .map_err(|e| e.to_string())
        ),
    );
    let looked_up_root = volume.lookup("/").and_then(|node| volume.metadata(&node));
    report.record(
        "file.lookup-root",
        matches!((&looked_up_root, &root_metadata), (Ok(a), Ok(b)) if a == b),
        describe(&looked_up_root),
    );

    let walk = walk_tree(volume, root, limits, &mut report);
    check_walked(volume, &walk, limits, &mut report);
    check_refusals(volume, &walk, &mut report);
    check_expected(volume, expected, &mut report);
    report
}

struct Walked<N> {
    /// For messages; lossy where a name is.
    path: String,
    /// The path as [`FileAPI::lookup_bytes`] takes it.
    bytes: Vec<u8>,
    entry: DirEntry<N>,
}

fn walk_tree<V: FileAPI>(
    volume: &V,
    root: V::Node,
    limits: &Limits,
    report: &mut Report,
) -> Vec<Walked<V::Node>> {
    let mut walked = Vec::new();
    let mut pending = vec![(String::new(), Vec::new(), root, 0usize)];
    let mut problems = Vec::new();
    let mut over_limit = false;
    while let Some((path, path_bytes, directory, depth)) = pending.pop() {
        let entries = match volume.read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                problems.push(format!("{path}/: {error}"));
                continue;
            }
        };
        let mut names = HashSet::new();
        for entry in entries {
            let name_ok = !entry.name.is_empty()
                && entry.name != "."
                && entry.name != ".."
                && !entry.name.contains(['/', '\0'])
                && !entry.bytes.is_empty()
                && entry.bytes != b"."
                && entry.bytes != b".."
                && !entry.bytes.iter().any(|byte| *byte == b'/' || *byte == 0);
            // The bytes and the name agree: the name is the bytes decoded,
            // exactly unless lossy.
            let agrees = match std::str::from_utf8(&entry.bytes) {
                Ok(text) => !entry.lossy && text == entry.name,
                Err(_) => entry.lossy,
            };
            if !name_ok || !agrees {
                problems.push(format!(
                    "{path}/: bad name {:?} (bytes {:?}, lossy {})",
                    entry.name, entry.bytes, entry.lossy
                ));
            }
            if !names.insert(entry.bytes.clone()) {
                problems.push(format!("{path}/: {:?} listed twice", entry.name));
            }
            if walked.len() >= limits.max_nodes {
                over_limit = true;
                break;
            }
            let child = format!("{path}/{}", entry.name);
            let mut child_bytes = path_bytes.clone();
            child_bytes.push(b'/');
            child_bytes.extend_from_slice(&entry.bytes);
            if entry.kind == NodeKind::Directory && name_ok {
                if depth + 1 > limits.max_depth {
                    over_limit = true;
                } else {
                    pending.push((
                        child.clone(),
                        child_bytes.clone(),
                        entry.node.clone(),
                        depth + 1,
                    ));
                }
            }
            walked.push(Walked {
                path: child,
                bytes: child_bytes,
                entry,
            });
        }
    }
    report.record(
        "file.tree-within-limits",
        !over_limit,
        format!("{} node(s), limits {limits:?}", walked.len()),
    );
    report.record(
        "file.directories-list",
        problems.is_empty(),
        format!("{problems:?}"),
    );
    walked
}

fn check_walked<V: FileAPI>(
    volume: &V,
    walk: &[Walked<V::Node>],
    limits: &Limits,
    report: &mut Report,
) {
    let mut metadata_problems = Vec::new();
    let mut read_problems = Vec::new();
    let mut lookup_problems = Vec::new();
    let mut files = 0usize;
    let mut looked_up = 0usize;
    let stride = walk.len().div_ceil(limits.max_lookups.max(1)).max(1);
    for (index, Walked { path, bytes, entry }) in walk.iter().enumerate() {
        let metadata = match volume.metadata(&entry.node) {
            Ok(metadata) => metadata,
            Err(error) => {
                metadata_problems.push(format!("{path}: {error}"));
                continue;
            }
        };
        if metadata.kind != entry.kind
            || (entry.kind == NodeKind::File && metadata.size != entry.size)
        {
            metadata_problems.push(format!(
                "{path}: listed {:?} {} bytes, metadata {:?} {} bytes",
                entry.kind, entry.size, metadata.kind, metadata.size
            ));
        }
        if index % stride == 0 || entry.lossy {
            looked_up += 1;
            match volume
                .lookup_bytes(bytes)
                .and_then(|node| volume.metadata(&node))
            {
                Ok(found) if found == metadata => {}
                other => lookup_problems.push(format!("{path}: {}", describe(&other))),
            }
        }
        if entry.kind == NodeKind::File {
            files += 1;
            if let Err(problem) = check_file_reads(volume, &entry.node, &metadata) {
                read_problems.push(format!("{path}: {problem}"));
            }
        }
    }
    report.record(
        "file.metadata-matches-listing",
        metadata_problems.is_empty(),
        format!("{} node(s); {metadata_problems:?}", walk.len()),
    );
    report.record(
        "file.paths-resolve",
        lookup_problems.is_empty(),
        format!(
            "{looked_up} of {} path(s) resolved; {lookup_problems:?}",
            walk.len()
        ),
    );
    report.record(
        "file.files-read-back",
        read_problems.is_empty(),
        format!("{files} file(s); {read_problems:?}"),
    );
}

/// A file copies exactly its size, reads 0 at its end, and a positioned read
/// from its middle agrees with the copy.
fn check_file_reads<V: FileAPI>(
    volume: &V,
    node: &V::Node,
    metadata: &Metadata,
) -> Result<(), String> {
    let mut copy = Vec::new();
    let copied = volume.copy_to(node, &mut copy).map_err(|e| e.to_string())?;
    if copied != metadata.size || copy.len() as u64 != metadata.size {
        return Err(format!("copied {copied} of {} bytes", metadata.size));
    }
    let mut probe = [0u8; 64];
    let want = metadata.size.min(probe.len() as u64) as usize;
    let first = volume
        .read_at(node, 0, &mut probe)
        .map_err(|e| e.to_string())?;
    if first != want || probe[..first] != copy[..first] {
        return Err(format!("read {first} of {want} byte(s) at 0"));
    }
    let at_end = volume
        .read_at(node, metadata.size, &mut probe)
        .map_err(|e| e.to_string())?;
    if at_end != 0 {
        return Err(format!("read {at_end} byte(s) at the end"));
    }
    if metadata.size > 1 {
        let middle = metadata.size / 2;
        let read = volume
            .read_at(node, middle, &mut probe)
            .map_err(|e| e.to_string())?;
        let want = (metadata.size - middle).min(probe.len() as u64) as usize;
        let from_copy = &copy[middle as usize..middle as usize + read.min(want)];
        if read != want || probe[..read] != *from_copy {
            return Err(format!(
                "read {read} byte(s) at {middle} disagree with the copy"
            ));
        }
    }
    Ok(())
}

fn check_refusals<V: FileAPI>(volume: &V, walk: &[Walked<V::Node>], report: &mut Report) {
    let absent = volume.lookup("/conformance-absent-7f3a9c1e");
    report.record(
        "file.lookup-absent-refused",
        absent.is_err(),
        describe(&absent),
    );
    if let Some(Walked { path, entry, .. }) = walk
        .iter()
        .find(|w| w.entry.kind == NodeKind::File && !w.entry.lossy)
    {
        let listed = volume.read_dir(&entry.node);
        report.record(
            "file.read-dir-of-file-refused",
            listed.is_err(),
            format!("{path}: {}", describe(&listed)),
        );
        let through = volume.lookup(&format!("{path}/x"));
        report.record(
            "file.lookup-through-file-refused",
            through.is_err(),
            format!("{path}/x: {}", describe(&through)),
        );
    }
    if let Some(Walked { path, entry, .. }) =
        walk.iter().find(|w| w.entry.kind == NodeKind::Directory)
    {
        let read = volume.read_at(&entry.node, 0, &mut [0u8; 1]);
        report.record(
            "file.read-of-directory-refused",
            read.is_err(),
            format!("{path}: {}", describe(&read)),
        );
    }
}

fn check_expected<V: FileAPI>(volume: &V, expected: &[Expected], report: &mut Report) {
    for item in expected {
        match item {
            Expected::File { path, contents } => {
                let mut copy = Vec::new();
                let read = volume
                    .lookup(path)
                    .and_then(|node| volume.copy_to(&node, &mut copy));
                report.record(
                    "file.expected-file",
                    read.is_ok() && copy == *contents,
                    format!(
                        "{path}: {}, {} of {} byte(s)",
                        describe(&read),
                        copy.len(),
                        contents.len()
                    ),
                );
            }
            Expected::Directory { path } => {
                let found = volume.lookup(path).and_then(|node| volume.metadata(&node));
                report.record(
                    "file.expected-directory",
                    matches!(&found, Ok(m) if m.kind == NodeKind::Directory),
                    format!("{path}: {}", describe(&found.map(|m| m.kind))),
                );
            }
            Expected::Absent { path } => {
                let found = volume.lookup(path);
                report.record(
                    "file.expected-absent",
                    found.is_err(),
                    format!("{path}: {}", describe(&found)),
                );
            }
        }
    }
}

/// Cross-checks the namespace against the salvage scan: every non-empty file
/// [`FileAPI`] lists has a live file candidate of the same name -- its own,
/// or one a live parent claim gives it -- and where one such candidate (of
/// the file's size, when the name is shared) has a complete walk claiming
/// the file's size, the bytes its plan rebuilds are the file's bytes.
///
/// Opt-in: it needs a format whose candidates or parent claims carry names.
/// Holes, uninitialized and encoded runs are rebuilt through the walk's
/// plan, so compressed and sparse files are compared too.
pub fn check_files_against_candidates<V: FileAPI + PrimitiveAPI>(
    volume: &V,
    limits: &Limits,
) -> Report {
    let mut report = Report::default();
    let Ok(root) = volume.root() else {
        report.record("cross.root", false, "no root");
        return report;
    };
    let walk = walk_tree(volume, root, limits, &mut report);
    let salvage = match volume.salvage() {
        Ok(salvage) => salvage,
        Err(error) => {
            report.record("cross.adapter", false, error.to_string());
            return report;
        }
    };
    let candidates = match salvage.all_candidates() {
        Ok(candidates) => candidates,
        Err(error) => {
            report.record("cross.scan", false, error.to_string());
            return report;
        }
    };
    // A candidate's names: its own, and those its live parent claims give
    // it, since an inode format keeps names in directory entries.
    let claims = salvage.rebuild_tree(&candidates).ok();
    let mut named: Vec<(Vec<u8>, usize)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.kind != CandidateKind::File || candidate.state != CandidateState::Live {
            continue;
        }
        let own = candidate.name.clone().map(String::into_bytes);
        let claimed = claims
            .as_ref()
            .and_then(|tree| tree.nodes.get(index))
            .into_iter()
            .flat_map(|node| node.parents.iter())
            .filter(|claim| claim.state == CandidateState::Live)
            .filter_map(|claim| claim.name.clone());
        for name in own.into_iter().chain(claimed) {
            named.push((name, index));
        }
    }
    let mut exact: std::collections::HashMap<&[u8], Vec<usize>> = Default::default();
    for (name, index) in &named {
        exact.entry(name.as_slice()).or_default().push(*index);
    }
    let mut missing = Vec::new();
    let mut different = Vec::new();
    let mut compared = 0usize;
    for Walked { path, entry, .. } in walk
        .iter()
        .filter(|w| w.entry.kind == NodeKind::File && w.entry.size > 0)
    {
        // Exact bytes first; the driver's own matching, such as case
        // folding, only when nothing matches exactly.
        let mut indices: Vec<usize> = match exact.get(entry.bytes.as_slice()) {
            Some(found) => found.clone(),
            None => named
                .iter()
                .filter(|(name, _)| volume.names_match_bytes(name, &entry.bytes))
                .map(|(_, index)| *index)
                .collect(),
        };
        indices.sort_unstable();
        indices.dedup();
        let mut matching: Vec<_> = indices.iter().map(|index| &candidates[*index]).collect();
        if matching.is_empty() {
            missing.push(path.clone());
            continue;
        }
        if matching.len() > 1 {
            matching.retain(|c| c.size == Some(entry.size));
        }
        let [candidate] = matching.as_slice() else {
            continue;
        };
        let Ok(walked) = salvage.walk_extents(candidate) else {
            continue;
        };
        if walked.end != WalkEnd::Complete || walked.claimed != entry.size {
            continue;
        }
        let plan = walked.plan();
        let mut through_salvage = Vec::new();
        let mut through_files = Vec::new();
        let salvaged = salvage.materialize(&plan, &mut through_salvage);
        let copied = volume.copy_to(&entry.node, &mut through_files);
        compared += 1;
        if salvaged.is_err() || copied.is_err() || through_salvage != through_files {
            different.push(path.clone());
        }
    }
    report.record(
        "cross.files-have-candidates",
        missing.is_empty(),
        format!("without a live candidate: {missing:?}"),
    );
    report.record(
        "cross.salvaged-bytes-match-files",
        different.is_empty(),
        format!("{compared} compared; differing: {different:?}"),
    );
    report
}

fn describe<T, E: fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("error: {error}"),
    }
}

fn describe_salvage<T>(result: &Result<T, SalvageError>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("error: {error:?}"),
    }
}
