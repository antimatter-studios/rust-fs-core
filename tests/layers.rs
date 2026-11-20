//! The three API layers, exercised through a toy format that implements all
//! of them, and the conformance checks proven able to fail.

#![cfg(feature = "layers")]

#[path = "support/toyfs.rs"]
mod toyfs;

use fs_core::layers::conformance::{self, Expected, Limits, Report};
use fs_core::layers::{
    check_bounds, ApiError, Damage, DamageKind, Extent, FileAPI, HealthState, NodeKind, OpenVolume,
    Plan, PlanMetadata, PlanStep, PrimitiveAPI, Recognition, Severity, VolumeAPI, VolumeHealth,
};
use toyfs::{big_contents, healthy, Bug, Memory, ToyVolume, BLOCK, MAGIC};

fn open() -> ToyVolume {
    ToyVolume::open(Memory(healthy())).expect("healthy image opens")
}

fn expected() -> Vec<Expected> {
    vec![
        Expected::file("/hello.txt", b"hello, world\n"),
        Expected::file("/HELLO.TXT", b"hello, world\n"),
        Expected::file("/big.bin", &big_contents()),
        Expected::file("/empty", b""),
        Expected::file("/docs/readme.md", b"# docs\n"),
        Expected::file("/docs/deep/leaf", b"leaf bytes"),
        Expected::directory("/docs"),
        Expected::directory("/docs/deep"),
        Expected::absent("/gone.txt"),
        Expected::absent("/docs/nope"),
    ]
}

#[test]
fn a_conforming_driver_passes_every_layer() {
    let volume = open();
    let report = conformance::check_healthy(&volume, &expected(), &Limits::default());
    report.assert_passed();
    assert!(report.checks.len() > 30, "{report}");
    let cross = conformance::check_files_against_candidates(&volume, &Limits::default());
    cross.assert_passed();
    // The encoded file and the one with a name that is not UTF-8 are among
    // those compared byte for byte.
    let compared = cross
        .checks
        .iter()
        .find(|check| check.name == "cross.salvaged-bytes-match-files")
        .unwrap();
    // Six plain or single-unit files, plus Btrfs's two shapes: a unit stored
    // larger than it decodes to, and two units back to back (#462, #463).
    assert!(compared.detail.starts_with("8 compared"), "{cross}");
}

#[test]
fn a_damaged_volume_opens_for_salvage_and_keeps_the_lower_layers() {
    let mut image = healthy();
    image[30] ^= 0xff; // the primary superblock's label, under its checksum
    assert!(ToyVolume::open(Memory(image.clone())).is_err());
    let probe = ToyVolume::probe(&Memory(image.clone())).unwrap();
    assert_eq!(probe.recognition, Recognition::Damaged);

    let volume = ToyVolume::open_for_salvage(Memory(image)).unwrap();
    let health = volume.health().unwrap();
    assert_eq!(health.state, HealthState::Damaged);
    assert_eq!(health.findings[0].kind, DamageKind::Checksum);
    let copies = volume.superblocks().unwrap();
    assert_eq!(copies[0].integrity, fs_core::layers::Integrity::Failed);
    assert_eq!(copies[1].integrity, fs_core::layers::Integrity::Verified);
    assert!(!volume.support().unwrap().files);
    conformance::check_damaged(&volume, &Limits::default()).assert_passed();
    // A healthy image's clean claim does not hold here, and the check says so.
    let clean = conformance::check_volume(&volume, true);
    assert_failed(&clean, "volume.health-clean");
    assert_failed(&clean, "volume.superblock-primary-valid");
    assert_failed(&clean, "volume.support-files-on-a-healthy-volume");
}

#[test]
fn probing_tells_a_volume_from_noise() {
    let yes = ToyVolume::probe(&Memory(healthy())).unwrap();
    assert_eq!(yes.recognition, Recognition::Yes);
    assert_eq!(yes.declared_bytes, Some(64 * BLOCK));
    let no = ToyVolume::probe(&Memory(vec![0; 4096])).unwrap();
    assert_eq!(no.recognition, Recognition::No);
    assert!(ToyVolume::probe(&Memory(vec![0; 10])).is_err());
}

fn assert_failed(report: &Report, check: &str) {
    let failed: Vec<&str> = report.failures().map(|c| c.name.as_str()).collect();
    assert!(
        failed.contains(&check),
        "expected {check} to fail; failures {failed:?}\n{report}"
    );
}

/// Each defect the toy can carry is caught by the check written for it.
#[test]
fn each_defect_fails_its_check() {
    let cases = [
        (Bug::ReadsPastEnd, "volume.read-past-end-refused"),
        (Bug::ShortFirstRead, "file.files-read-back"),
        (Bug::FlagsLie, "primitive.damage-flags"),
        (Bug::ScanMiscounts, "primitive.scan-count"),
        (Bug::TreeReversed, "primitive.tree-one-node-per-candidate"),
        (Bug::DuplicateEntry, "file.directories-list"),
        (Bug::EncodedUndecoded, "primitive.walks"),
        (Bug::LossyBytes, "file.directories-list"),
    ];
    for (bug, check) in cases {
        let volume = open().with_bug(bug);
        let report = conformance::check_healthy(&volume, &expected(), &Limits::default());
        assert!(!report.passed(), "{bug:?} passed");
        assert_failed(&report, check);
    }
    let mut image = healthy();
    image[30] ^= 0xff;
    let hiding = ToyVolume::open_for_salvage(Memory(image))
        .unwrap()
        .with_bug(Bug::HealthHidesDamage);
    assert_failed(
        &conformance::check_damaged(&hiding, &Limits::default()),
        "volume.health-consistent",
    );
}

#[test]
fn wrong_expectations_and_tight_limits_fail() {
    let volume = open();
    let wrong = [
        Expected::file("/hello.txt", b"goodbye"),
        Expected::directory("/hello.txt"),
        Expected::absent("/docs"),
    ];
    let report = conformance::check_files(&volume, &wrong, &Limits::default());
    for check in [
        "file.expected-file",
        "file.expected-directory",
        "file.expected-absent",
    ] {
        assert_failed(&report, check);
    }
    let tight = Limits {
        max_nodes: 2,
        ..Limits::default()
    };
    assert_failed(
        &conformance::check_files(&volume, &[], &tight),
        "file.tree-within-limits",
    );
    let shallow = Limits {
        max_depth: 0,
        ..Limits::default()
    };
    assert_failed(
        &conformance::check_files(&volume, &[], &shallow),
        "file.tree-within-limits",
    );
    let few_walks = Limits {
        max_walks: 1,
        ..Limits::default()
    };
    assert_failed(
        &conformance::check_primitive(&volume, &few_walks),
        "primitive.walk-limit",
    );
}

#[test]
fn an_empty_report_never_passes() {
    let report = Report::default();
    assert!(!report.passed());
    assert_eq!(report.to_string(), "no checks ran\n");
    let caught = std::panic::catch_unwind(|| report.assert_passed());
    assert!(caught.is_err());
}

#[test]
fn paths_resolve_case_insensitively_where_the_driver_says_so() {
    let volume = open();
    let node = volume.lookup("//DOCS//Deep/LEAF").unwrap();
    let mut out = Vec::new();
    assert_eq!(volume.copy_to(&node, &mut out).unwrap(), 10);
    assert_eq!(out, b"leaf bytes");
    let through_file = volume.lookup("/hello.txt/x").unwrap_err();
    assert!(
        matches!(through_file, toyfs::ToyError::Api(ApiError::NotADirectory(ref p)) if p == "/hello.txt")
    );
    let missing = volume.lookup("/docs/missing").unwrap_err();
    assert!(
        matches!(missing, toyfs::ToyError::Api(ApiError::NotFound(ref p)) if p == "/docs/missing")
    );
    let root = volume.root().unwrap();
    assert!(volume.read_link(&root).is_err());
}

#[test]
fn copying_into_a_failing_sink_reports_the_sink() {
    struct Full;
    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let volume = open();
    let node = volume.lookup("/big.bin").unwrap();
    let error = volume.copy_to(&node, &mut Full).unwrap_err();
    assert!(matches!(error, toyfs::ToyError::Api(ApiError::Sink(_))));
    assert!(error.to_string().contains("full"));
}

#[test]
fn bounds_are_checked_without_overflow() {
    assert!(check_bounds(0, 10, 10).is_ok());
    assert!(check_bounds(10, 0, 10).is_ok());
    let past = check_bounds(5, 6, 10).unwrap_err();
    assert_eq!(
        past.to_string(),
        "6 byte(s) at 5 lie outside the 10-byte volume"
    );
    assert!(check_bounds(u64::MAX, 1, u64::MAX).is_err());
}

#[test]
fn api_errors_say_what_happened() {
    let cases: [(ApiError, &str); 5] = [
        (ApiError::Unsupported("links"), "unsupported: links"),
        (ApiError::NotFound("/a".into()), "not found: /a"),
        (ApiError::NotADirectory("/a".into()), "not a directory: /a"),
        (ApiError::IsADirectory("/a".into()), "is a directory: /a"),
        (
            ApiError::Sink(std::io::Error::other("disk full")),
            "output sink failed: disk full",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
        let has_source = std::error::Error::source(&error).is_some();
        assert_eq!(has_source, matches!(error, ApiError::Sink(_)));
    }
}

#[test]
fn health_follows_the_worst_finding() {
    let finding = |severity| Damage {
        kind: DamageKind::Recorded,
        severity,
        location: Extent::new(0, 1),
        structure: "flags".into(),
        detail: String::new(),
    };
    assert!(VolumeHealth::from_findings(Vec::new()).is_clean());
    assert_eq!(
        VolumeHealth::from_findings(vec![finding(Severity::Warning)]).state,
        HealthState::Degraded,
        "a dead backup is not clean"
    );
    assert_eq!(
        VolumeHealth::from_findings(vec![finding(Severity::Notice)]).state,
        HealthState::NeedsRecovery
    );
    assert_eq!(
        VolumeHealth::from_findings(vec![finding(Severity::Notice), finding(Severity::Error)])
            .state,
        HealthState::Damaged
    );
}

#[test]
fn the_bound_salvage_verbs_read_the_volume_source() {
    let volume = open();
    let salvage = volume.salvage().unwrap();
    assert!(salvage.capabilities().missing.is_empty());
    assert_eq!(salvage.source().size_bytes(), volume.size_bytes());
    let candidates = salvage.all_candidates().unwrap();
    let names: Vec<_> = candidates.iter().filter_map(|c| c.name.clone()).collect();
    assert!(names.contains(&"gone.txt".to_owned()), "{names:?}");
    let leaf = candidates
        .iter()
        .find(|c| c.name.as_deref() == Some("leaf"))
        .unwrap();
    let walk = salvage.walk_extents(leaf).unwrap();
    let plan = walk.plan();
    let reader = salvage.plan_reader(&plan).unwrap();
    let mut out = [0u8; 10];
    reader.read_at(0, &mut out).unwrap();
    assert_eq!(&out, b"leaf bytes");
    assert_eq!(salvage.check_plan(&plan).unwrap().length, 10);
    let mut found = Vec::new();
    assert_eq!(salvage.report_damage(&mut found).unwrap().reported, 0);
    let mut header = [0u8; 8];
    salvage
        .read_extent(Extent::new(0, 8), 0, &mut header)
        .unwrap();
    assert_eq!(&header, MAGIC);
    let tree = salvage.rebuild_tree(&candidates).unwrap();
    assert_eq!(tree.orphans().count(), 0);
    let structures = volume.structures().unwrap();
    assert_eq!(structures.len(), 3);
    let mut materialized = Vec::new();
    salvage.materialize(&plan, &mut materialized).unwrap();
    assert_eq!(materialized, b"leaf bytes");
    assert_eq!(volume.superblock().unwrap().label, "TOYVOL");
    assert_eq!(volume.format(), "toyfs");
    assert_eq!(
        volume
            .metadata(&volume.lookup("/docs").unwrap())
            .unwrap()
            .kind,
        NodeKind::Directory
    );
}

type Evidence = (
    VolumeHealth,
    fs_core::layers::Geometry,
    Vec<fs_core::layers::Structure>,
    Vec<(String, NodeKind, u64)>,
);

#[test]
fn evidence_round_trips_through_json() {
    let volume = open();
    let health = volume.health().unwrap();
    let geometry = volume.geometry().unwrap();
    let structures = volume.structures().unwrap();
    let root = volume.root().unwrap();
    let listing: Vec<_> = volume
        .read_dir(&root)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.name, entry.kind, entry.size))
        .collect();
    let json = serde_json::to_string(&(&health, &geometry, &structures, &listing)).unwrap();
    assert!(json.contains("\"allocationUnit\":512"), "{json}");
    assert!(json.contains("\"superblock\""), "{json}");
    let back: Evidence = serde_json::from_str(&json).unwrap();
    assert_eq!(back.0, health);
    assert_eq!(back.1, geometry);
    assert_eq!(back.2, structures);
    assert_eq!(back.3, listing);
}

// -------------------------------------------------- plans (from exFAT) ---

fn pinned() -> Plan {
    let mut plan = Plan::new(vec![
        PlanStep::Extent(Extent::new(28672, 4)),
        PlanStep::Zeros(2),
        PlanStep::Bytes(vec![1, 2]),
        PlanStep::Clusters {
            first: 70,
            count: 1,
        },
    ]);
    plan.length = Some(4100);
    plan.valid_length = Some(4000);
    plan.metadata = PlanMetadata {
        name: Some("short.bin".into()),
        created: Some("2024-01-02 03:04:05 +00:00".into()),
        modified: None,
        accessed: Some(String::new()),
    };
    plan
}

// The values rust-fs-exfat pinned before the plan moved here: a plan's
// identity is unchanged by the move.
const PINNED_BYTES: &str = "66732d73616c766167652d706c616e01040000000000000001007000000000000004000000000000000202000000000000000302000000000000000102044600000000000000010000000000000001041000000000000001a00f00000000000001090000000000000073686f72742e62696e011a00000000000000323032342d30312d30322030333a30343a3035202b30303a303000010000000000000000";
const PINNED_DIGEST: &str = "68fd79115f94e496050ab614994f863e4e7ce56ae2aa38f4dea80193799ff020";

#[test]
fn a_plan_digest_is_unchanged_by_the_move_from_exfat() {
    let plan = pinned();
    let hex: String = plan
        .canonical_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(hex, PINNED_BYTES);
    assert_eq!(plan.digest(), PINNED_DIGEST);
    let back: Plan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(back.digest(), PINNED_DIGEST);
}

#[test]
fn plans_resolve_lengths_and_refuse_what_they_cannot_execute() {
    let volume = open();
    let salvage = volume.salvage().unwrap();
    let mut plan = Plan::new(vec![PlanStep::Clusters { first: 3, count: 1 }]);
    assert_eq!(plan.length(), None);
    plan.length = Some(13);
    let mut out = Vec::new();
    let report = salvage.materialize(&plan, &mut out).unwrap();
    assert_eq!(out, b"hello, world\n");
    assert_eq!(report.source_of(0), Some(3 * BLOCK));
    plan.length = Some(600);
    assert!(salvage.check_plan(&plan).is_err());
    plan.length = None;
    plan.valid_length = Some(1000);
    assert!(salvage.check_plan(&plan).is_err());
    let outside = Plan::new(vec![PlanStep::Clusters {
        first: 64,
        count: 1,
    }]);
    assert!(salvage.check_plan(&outside).is_err());
    let overlapping = Plan::new(vec![
        PlanStep::Extent(Extent::new(0, 100)),
        PlanStep::Extent(Extent::new(50, 100)),
    ]);
    let check = salvage.check_plan(&overlapping).unwrap();
    assert_eq!(check.overlaps.len(), 1);
    assert_eq!(check.overlaps[0].source, Extent::new(50, 50));
    assert_eq!(
        Plan::new(vec![PlanStep::Zeros(5), PlanStep::Bytes(vec![1])])
            .validate(0)
            .unwrap(),
        6
    );
}

#[test]
fn a_walk_with_holes_and_uninitialized_runs_plans_the_file_in_place() {
    use fs_core::layers::{ExtentWalk, WalkEnd, WalkExtent};
    let walk = ExtentWalk {
        extents: vec![
            WalkExtent {
                offset: 4096,
                source: Extent::new(8192, 100),
                uninitialized: false,
                encoded: None,
            },
            WalkExtent {
                offset: 8192,
                source: Extent::new(16384, 50),
                uninitialized: true,
                encoded: None,
            },
        ],
        claimed: 10000,
        end: WalkEnd::Complete,
    };
    assert_eq!(walk.mapped(), 150);
    let plan = walk.plan();
    assert_eq!(
        plan.steps,
        vec![
            PlanStep::Zeros(4096),
            PlanStep::Extent(Extent::new(8192, 100)),
            PlanStep::Zeros(3996),
            PlanStep::Zeros(50),
            PlanStep::Zeros(10000 - 8242),
        ]
    );
    assert_eq!(plan.length(), Some(10000));
    let broken = ExtentWalk {
        end: WalkEnd::Broken,
        ..walk.clone()
    };
    assert_eq!(
        broken.plan().length(),
        Some(8242),
        "a broken walk plans what it reached"
    );
    let json = serde_json::to_string(&walk).unwrap();
    assert_eq!(serde_json::from_str::<ExtentWalk>(&json).unwrap(), walk);
    let contiguous = ExtentWalk::contiguous(
        vec![Extent::new(512, 512), Extent::new(4096, 10)],
        522,
        WalkEnd::Complete,
    );
    assert_eq!(contiguous.extents[1].offset, 512);
    assert_eq!(contiguous.plan().length(), Some(522));
}

#[test]
fn parent_claims_carry_the_name_and_state_of_the_entry_making_them() {
    let volume = open();
    let salvage = volume.salvage().unwrap();
    let candidates = salvage.all_candidates().unwrap();
    let tree = salvage.rebuild_tree(&candidates).unwrap();
    let leaf = candidates
        .iter()
        .position(|c| c.name.as_deref() == Some("leaf"))
        .unwrap();
    let claim = &tree.nodes[leaf].parents[0];
    assert_eq!(claim.name.as_deref(), Some(&b"leaf"[..]));
    assert_eq!(claim.state, fs_core::layers::CandidateState::Live);
    let gone = candidates
        .iter()
        .position(|c| c.name.as_deref() == Some("gone.txt"))
        .unwrap();
    assert_eq!(
        tree.nodes[gone].parents[0].state,
        fs_core::layers::CandidateState::Deleted
    );
}

#[test]
fn reading_an_extent_at_its_end_reads_nothing() {
    let volume = open();
    let salvage = volume.salvage().unwrap();
    let mut buffer = [0u8; 4];
    assert_eq!(
        salvage
            .read_extent(Extent::new(0, 8), 8, &mut buffer)
            .unwrap(),
        0
    );
}

#[test]
fn a_name_that_is_not_utf8_is_found_by_its_bytes() {
    let volume = open();
    let node = volume.lookup_bytes(b"/docs/deep/caf\xe9").unwrap();
    let mut out = Vec::new();
    volume.copy_to(&node, &mut out).unwrap();
    assert_eq!(out, b"latin-1 name");
    assert!(
        volume.lookup("/docs/deep/caf\u{e9}").is_err(),
        "UTF-8 é is other bytes"
    );
    let deep = volume.lookup("/docs/deep").unwrap();
    let entry = volume
        .read_dir(&deep)
        .unwrap()
        .into_iter()
        .find(|entry| entry.lossy)
        .unwrap();
    assert_eq!(entry.bytes, b"caf\xe9");
    assert_eq!(entry.name, "caf\u{fffd}");
    // Case folding still applies to names that are UTF-8.
    assert!(volume.lookup_bytes(b"/DOCS/README.MD").is_ok());
}

#[test]
fn an_encoded_file_walks_as_stored_bytes_and_plans_as_decoded_ones() {
    use fs_core::layers::Feature;
    let volume = open();
    let salvage = volume.salvage().unwrap();
    let candidates = salvage.all_candidates().unwrap();
    let packed = candidates
        .iter()
        .find(|c| c.name.as_deref() == Some("packed.txt"))
        .unwrap();
    let walk = salvage.walk_extents(packed).unwrap();
    let encoding = walk.extents[0].encoded.clone().unwrap();
    assert_eq!(encoding.codec, "reverse");
    assert_eq!(walk.extents[0].length(), 30);
    let plan = walk.plan();
    assert!(matches!(plan.steps[0], PlanStep::Encoded { .. }));
    let mut out = Vec::new();
    let report = salvage.materialize(&plan, &mut out).unwrap();
    assert_eq!(out, b"stored reversed, read forwards");
    assert!(report.ranges[0].encoded);
    assert_eq!(
        report.source_of(0),
        None,
        "decoded bytes map to no one source byte"
    );
    let reader = salvage.plan_reader(&plan).unwrap();
    let mut middle = [0u8; 8];
    reader.read_at(7, &mut middle).unwrap();
    assert_eq!(&middle, b"reversed");
    assert_eq!(reader.source_of(0), None);
    let mut decoded = [0u8; 6];
    assert_eq!(
        salvage
            .read_encoded(walk.extents[0].source, &encoding, 0, &mut decoded)
            .unwrap(),
        6
    );
    assert_eq!(&decoded, b"stored");
    let refused = salvage.read_encoded(
        walk.extents[0].source,
        &fs_core::layers::Encoding::whole("zstd", 30),
        0,
        &mut decoded,
    );
    assert!(matches!(
        refused,
        Err(fs_core::layers::SalvageError::Unsupported(
            Feature::EncodedExtents
        ))
    ));
    // An encoded step past the source is refused before anything is read.
    let outside = Plan::new(vec![PlanStep::Encoded {
        source: Extent::new(64 * BLOCK, 1),
        encoding: fs_core::layers::Encoding::whole("reverse", 1),
    }]);
    // A run that is the middle of what its stored bytes decode to.
    let middle = Plan::new(vec![PlanStep::Encoded {
        source: walk.extents[0].source,
        encoding: fs_core::layers::Encoding {
            codec: "reverse".into(),
            decoded: 30,
            offset: 7,
            length: 8,
        },
    }]);
    let mut run = Vec::new();
    salvage.materialize(&middle, &mut run).unwrap();
    assert_eq!(run, b"reversed");
    assert!(salvage.check_plan(&outside).is_err());
    // A plan without encoded steps keeps its digest; one with them is new.
    assert_eq!(pinned().digest(), PINNED_DIGEST);
    assert_ne!(plan.digest(), Plan::new(Vec::new()).digest());
    let json = serde_json::to_string(&plan).unwrap();
    assert_eq!(
        serde_json::from_str::<Plan>(&json).unwrap().digest(),
        plan.digest()
    );
}

#[test]
fn a_geometry_hypothesis_builds_an_adapter_without_the_superblock() {
    let volume = open();
    let hypotheses = volume.hypotheses().unwrap();
    assert_eq!(hypotheses, [64]);
    let salvage = volume.salvage_with(&hypotheses[0]).unwrap();
    let candidates = salvage.all_candidates().unwrap();
    assert!(candidates.iter().any(|c| c.name.as_deref() == Some("leaf")));
    let layout = fs_core::layers::SalvageAdapter::cluster_layout(salvage.adapter()).unwrap();
    assert_eq!(layout.count, 64);
}
