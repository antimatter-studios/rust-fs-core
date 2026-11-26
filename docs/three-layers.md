# The three driver API layers

Every rust-fs driver offers the same three layers, defined once in this crate
under `fs_core::layers` (cargo feature `layers`). A consumer that knows the
traits can open, inspect, salvage and browse any format without knowing which
one it is. The format's knowledge stays in the driver, below the traits.

```
            FileAPI                 PrimitiveAPI
   browse, read, copy files     structures, salvage verbs
   (healthy namespace)          (read-only, damage-tolerant)
              \                     /
               \                   /
                     VolumeAPI
        open, bytes, geometry, superblock, health
                         |
                    BlockRead source
```

## Generic in core, specific in the driver

Only what is generic across filesystems lives in rust-fs-core: the traits,
the evidence and plan types, the salvage verbs and the conformance helper.
What is specific to a format -- FAT chains, exFAT entry sets, NTFS records,
ext4 inodes and extent trees, XFS AG B-trees, Btrfs and bcachefs tree nodes,
EROFS inodes, SquashFS metablocks -- stays in its driver, reachable through
that driver's own `PrimitiveAPI` implementation. Every driver implements the
salvage verbs, damage-tolerant, over its own primitives.

## Read-only by type

Every call that touches the source takes it as `BlockRead`, which has no
write method, or reaches it through a volume that holds only a `BlockRead`.
No layer takes a device that can write, a path, or a handle that could
become one. A runtime flag is not the guarantee; the parameter type is.

## How the layers fit

`PrimitiveAPI` and `FileAPI` both require `VolumeAPI`. Neither requires the
other, so a damaged volume opened for salvage offers the primitives without
a namespace, and a read-only format with no deleted state still offers both.

Nothing in any layer writes. A driver that can write keeps that surface as
its own API; the shared traits are read-only by type, because the source
they reach is a `BlockRead`, which has no write method.

## VolumeAPI -- `fs_core::layers::volume`

| Item | Contract |
|---|---|
| `type Error` | The driver's error. It converts from `ApiError` (the shared refusals) and from `fs_core::Error` (device errors). |
| `type Superblock` | The driver's decoded boot sector or superblock, as plain data. |
| `format()` | The format's lowercase name: `fat32`, `exfat`, `ntfs`, `ext4`. |
| `size_bytes()` | Bytes the volume can read. |
| `read_volume_bytes(offset, output)` | Volume-relative bytes. Out of bounds is `ApiError::Bounds` (use `check_bounds`); never a short read. |
| `geometry()` | The format-neutral `Geometry`: sector size, allocation unit and count, declared length, the linear `ClusterLayout` when the format has one, label and serial. |
| `superblock()` | The decoded `Superblock` the volume was opened with. |
| `superblocks()` | Every copy the format keeps, primary first, as `SuperblockCopy`: copy number, location, the decoded copy where it decodes, and its integrity. Damaged copies are listed, never left out. |
| `support()` | `VolumeSupport` for this volume: whether `FileAPI` can browse it, the salvage features the driver lacks (equal to its adapter's capabilities), and the format features this volume uses that the driver cannot read. |
| `health()` | The quick `VolumeHealth` verdict at open, with the `Damage` findings behind it. `VolumeHealth::from_findings` derives the state, worst first: an error finding makes it `Damaged`, a notice `NeedsRecovery` (dirty flag, journal to replay), a warning `Degraded` (a redundant copy is damaged, such as a dead backup superblock), and no findings `Clean`. `Unknown` when the driver cannot tell. A clean volume carries no findings. Quick: it reads what the driver reads to open (superblock copies, descriptor tables, the root) and can miss deep damage, such as a zeroed subdirectory block. A consumer does not show `Clean` as "no damage"; `report_damage` is the full scan. |

`OpenVolume` (kept apart so `VolumeAPI` stays usable as a trait object):

| Item | Contract |
|---|---|
| `type Source: BlockRead` | The device the volume holds. |
| `probe(&dyn BlockRead)` | Recognition without opening: `Recognition::Yes`, `Damaged` (worth a salvage open) or `No`, with the declared length. |
| `open(source)` | A volume whose structures validate, ready for `FileAPI`. |
| `open_for_salvage(source)` | Only what salvage needs, usually a usable geometry from any copy. Defaults to `open`. |

## PrimitiveAPI -- `fs_core::layers::primitive`, `fs_core::layers::salvage`

| Item | Contract |
|---|---|
| `type Adapter: SalvageAdapter` | The format's salvage adapter: format context only, never a device. |
| `source()` | The volume's source, lent read-only. |
| `type Hypothesis` | What a caller may suppose about the geometry when the structures recording it are gone (block size, metadata start), in the format's own terms; plain serde data. `()` for a driver that never needs one. |
| `adapter()` | The adapter for this volume, from whatever geometry survived; `SalvageError::NoGeometry` when none did. |
| `hypotheses()` | Plausible hypotheses the driver infers from the surviving bytes, most likely first. Empty by default. |
| `adapter_with(hypothesis)` / `salvage_with(hypothesis)` | The adapter built from a caller's hypothesis instead of the superblock, for a source whose superblock is gone. Results are evidence under that hypothesis. Defaults to `Unsupported(GeometryHypotheses)`. |
| `structures()` | Every structure at a known location -- boot sectors, superblocks and their backups, allocation maps, record tables, directory roots, journals -- each with role, location, copy number and integrity. Damaged ones are listed, never left out. |
| `salvage()` | The adapter bound to the source: a `Salvage` whose methods are the salvage verbs without the source argument. |

The format's own primitives -- FAT entries and chains, MFT records and
attribute streams, inodes, B-tree nodes -- stay in the driver, as inherent
methods or a driver trait that extends `PrimitiveAPI`.

`SalvageAdapter` is the salvage-verb trait first written in rust-fs-exfat,
moved here and extended where ext4 and EROFS did not fit:

| Verb | Method | Result |
|---|---|---|
| Find candidates | `find_candidates(source, range, sink)` | `Candidate`s to the sink, then a `ScanReport` with unreadable ranges and a resume offset |
| Identify a candidate | `candidate_id(candidate)` | The id trees and parent claims name: `location.offset` by default; a driver whose records share source bytes (SquashFS inodes packed in one compressed metadata block) overrides it, with the inode reference for SquashFS. A scan stopped among a shared record's candidates resumes at the record and delivers them again; callers drop repeats by id. |
| Walk chains or extents | `walk_extents(source, candidate)` | `ExtentWalk`: file-ordered `WalkExtent`s -- file offset, source extent, `uninitialized`, and an `Encoding` when the source bytes are compressed: the codec, the size the stored unit decodes to, and which of those decoded bytes are this run's (one EROFS pcluster or SquashFS fragment block serves several runs) -- with gaps as holes, and why the walk ended. `claimed` is in file bytes; `mapped()` counts source bytes as stored. `ExtentWalk::plan()` turns it into a `Plan` that rebuilds the file in place: zeros for holes and uninitialized runs, `PlanStep::Encoded` for compressed runs. `ExtentWalk::contiguous` builds a hole-free walk (FAT, exFAT chains). |
| Rebuild the tree | `rebuild_tree(source, candidates)` | `Tree`: every `ParentClaim` per candidate, with the name the claiming record gives it (as bytes, by the rule `DirEntry::bytes` follows) and that record's state. A hard-linked inode has one claim per link; a deleted entry's claim is weaker evidence than a live one. |
| Report damage | `report_damage(source, sink)` | `Damage`s to the sink, then a `DamageReport` |
| Read by extent | `read_extent(source, extent, offset, output)` | Source bytes as stored: `Ok(0)` at the extent's end, `Bounds` past it |
| Decode an extent | `read_encoded(source, stored, encoding, offset, output)` | The run's file bytes from an encoded walk extent, `offset` into the run, `Ok(0)` at its end. Bytes that do not decode are an error, never guessed. Defaults to `Unsupported(EncodedExtents)`. |
| Materialize from a plan | `materialize`, `plan_reader`, `check_plan` | Output bytes with the provenance of every byte. An encoded step's output range is flagged `encoded` and names its stored bytes; `source_of` gives no single source byte for it. |

An adapter holds format context, never a device, and decodes only from
the source each call is given. It may cache what it decoded from a source
-- a directory index, a name table -- only keyed on that source's identity
and on a checksum of the structures the cache was built from, so an adapter
built from one source decodes nothing from another, and a cache never
outlives a change to the bytes it came from.

`capabilities()` names the `Feature`s a driver lacks. A missing feature is
refused with `SalvageError::Unsupported(feature)`; an empty result always
means nothing was found. A read-only format (squashfs, erofs) lists
`DeletedCandidates` and `DeletedExtents`. A format without a linear
numbering of allocation units (btrfs, bcachefs) lists `ClusterSteps`. A
format that stores nothing compressed lists `EncodedExtents`.

A format with no record table -- EROFS, which finds inodes by scanning --
needs a plausibility filter on what it reports as a candidate (decodes, has
a link, names a file type, reserved fields zero), strong enough that a
healthy image shows no failed candidates. Otherwise slots inside data blocks
surface as damaged candidates, and the salvage set's `Reported` rule
rejects them.

Evidence -- candidates, walks, trees, damage, plans, reports, geometry,
health, structures -- is plain serde data with no handles, so it can be
stored, sent and compared. A plan's `digest()` is stable across platforms
and releases; it is the same value rust-fs-exfat computed before the move.
`PlanStep::Encoded` has its own tag in the canonical encoding, so a plan
without one digests exactly as before.

## FileAPI -- `fs_core::layers::file`

A driver implements four methods over a node handle of its choosing (inode
number, MFT reference with sequence, directory-entry location):

| Item | Contract |
|---|---|
| `type Node` | The handle, valid for the volume that issued it. |
| `root()` | The root directory. |
| `read_dir(node)` | Entries without `.` and `..`; `ApiError::NotADirectory` on anything else. Each `DirEntry` has a decoded `name`, its `bytes` (the stored bytes on a byte-named format, the UTF-8 of `name` on FAT, exFAT and NTFS), `lossy` when the bytes do not decode, kind, size and node. |
| `metadata(node)` | `Metadata`: kind, size and, where the format records them, allocated bytes, times, mode, links and attribute bits. |
| `read_at(node, offset, output)` | Exactly `min(bytes left, output.len())` bytes; 0 at or past the end; `ApiError::IsADirectory` on a directory. |

Provided on top, the same for every driver:

| Item | Contract |
|---|---|
| `lookup_bytes(path)` | Absolute `/`-separated byte paths, empty components ignored, one `read_dir` per component, matched on `DirEntry::bytes`. A name that is not UTF-8 is found this way. |
| `lookup(path)` | `lookup_bytes` over the path's UTF-8. |
| `names_match(stored, wanted)` | Exact by default. FAT, exFAT and NTFS override it with their case folding. |
| `names_match_bytes(stored, wanted)` | `names_match` when both are UTF-8, byte for byte otherwise. |
| `copy_to(node, sink)` | Exactly the file's size to an `io::Write`, or an error. |
| `read_link(node)` | `ApiError::Unsupported` unless the format has links. |

## Conformance -- `fs_core::layers::conformance`

A driver's tests open each of its images and assert the report:

```rust
use fs_core::layers::conformance::{self, Expected, Limits};

let volume = MyVolume::open(device)?;
conformance::check_healthy(
    &volume,
    &[Expected::file("/hello.txt", b"hello\n"), Expected::directory("/docs")],
    &Limits::default(),
)
.assert_passed();

let damaged = MyVolume::open_for_salvage(damaged_device)?;
conformance::check_damaged(&damaged, &Limits::default()).assert_passed();
```

| Function | Checks |
|---|---|
| `check_volume` | Format name, bounded reads (end, straddle, `u64::MAX`), geometry units and layout, superblock, health consistent with its findings, and clean when asked. |
| `check_primitive` | Adapter builds; structures listed, include a boot sector or superblock, and read alike through both layers; scan counts, bounds, stop and resume; walks stay on the source, keep file order, end within the claim, plan to exactly the claim when complete, and every encoded extent decodes to its length; hypotheses build adapters; trees keep one node per candidate in order; damage counts; extent reads refuse out of bounds; materialization returns the source's bytes; cluster layout agrees with geometry. Every missing feature must refuse as flagged, and every present one must run. |
| `check_files` | The whole tree walks within the limits; names are valid and unique, and agree with their bytes; metadata matches listings; every path resolves by its bytes, non-UTF-8 names included; every file copies exactly its size, reads exactly at 0 and the middle, and 0 at its end; refusals for absent paths, paths through files, listing a file and reading a directory; and the image holds the `Expected` files, directories and absences. |
| `check_files_against_candidates` | Opt-in, for formats whose candidates carry names: every file has a live candidate, and where one candidate's complete walk claims the file's size, the bytes its plan rebuilds -- holes, uninitialized and encoded runs included -- equal the file's bytes. |
| `salvage::run` | The deeper salvage set from rust-fs-exfat, over driver-built images with declared damage, unreadable sectors and strict-API rejection. |
| `salvage::run_sources` | The same set over sources a driver opens, each damaged case usually a `salvage::Overlay` of its damage on the healthy source, so no image is held in memory. |

The report fails when nothing ran, and a tree larger than the `Limits`
fails rather than being checked in part: raise `max_nodes` and `max_walks`
for a large fixture (build `Limits { max_nodes: 100_000, ..Limits::default() }`).
Every check over a walked entry is linear, except path resolution, which
reads every directory on the path: `max_lookups` spreads that many lookups
evenly over the tree, always including names that are not UTF-8, and the
report says how many ran. Conformance proves a driver keeps
the contract's shape; whether it reads the format correctly still needs an
independent oracle.

## Known limits

- One source per volume. An `Extent` names bytes on the volume's one
  `BlockRead`, so multi-device EROFS images and multi-device Btrfs walks end
  `Broken` at the first extent on another device (#460).
- `Geometry::sector_size` means little for an image format (EROFS,
  SquashFS); such a driver reports 512, or its read granularity, with the
  allocation unit equal to its block size.
- Alternate copies of a structure (journal and log copies, a FAT mirror) have
  no shared listing yet (#457).

## How the formats map

| Format | VolumeAPI superblock | Allocation unit / layout | PrimitiveAPI structures | Missing salvage features |
|---|---|---|---|---|
| FAT12/16/32 | boot sector, FSInfo | cluster, linear from the data region | boot + backup, FSInfo, each FAT copy, root directory | none |
| exFAT | boot region | cluster, linear | main + backup boot region, FATs, allocation bitmap, up-case table, root | none |
| NTFS | boot sector | cluster, linear from 0 | boot + backup, `$MFT`, `$MFTMirr`, `$LogFile`, `$Bitmap` | per driver |
| ext4 | superblock | block, linear from 0 | superblock + backups, group descriptors, inode tables, bitmaps, journal | per driver |
| XFS | superblock | block, linear per AG | AG headers (SB, AGF, AGI, AGFL), log | per driver |
| Btrfs | superblock | sector; chunk-mapped, `layout` is `None` | superblock mirrors, chunk tree, root tree | `ClusterSteps` |
| bcachefs | superblock | bucket; mapped, `layout` is `None` | superblock + backups, journal, btree roots | `ClusterSteps` |
| EROFS | superblock | block | superblock, inode area | `DeletedCandidates`, `DeletedExtents` (compressed pclusters are encoded extents) |
| SquashFS | superblock | block (compressed data) | superblock, inode/directory/fragment/id tables | `DeletedCandidates`, `DeletedExtents` (compressed blocks are encoded extents) |

A candidate's `Detail` must be `Serialize + DeserializeOwned`, so a driver
adds its own `serde = { version = "1", features = ["derive"] }` dependency
(MIT/Apache-2.0) to derive them.

## Depending on rust-fs-core: the pinning rule

A driver's **published** manifest names rust-fs-core by version (crates.io)
or by git tag, never by path:

```toml
[dependencies]
rust-fs-core = { version = "0.5", features = ["layers"] }
# or, before a crates.io release is available:
rust-fs-core = { git = "https://github.com/antimatter-studios/rust-fs-core", tag = "v0.5.0", features = ["layers"] }
```

`path = "../rust-fs-core"` must not appear in a manifest that is published
or tagged: a path does not exist on anyone else's machine, and a build that
depends on whatever happens to sit beside it is not reproducible.

Local development against a working tree of rust-fs-core uses a `[patch]`
that lives **outside** the published manifest -- in `.cargo/config.toml`
(ignored by git) or a workspace manifest that is never published:

```toml
# .cargo/config.toml -- not committed
[patch.crates-io]
rust-fs-core = { path = "../rust-fs-core" }
# for a git dependency:
[patch."https://github.com/antimatter-studios/rust-fs-core"]
rust-fs-core = { path = "../rust-fs-core" }
```

The lock file then records the published version or the tag's commit, and
CI builds what was published. Removing the patch is all it takes to build
what a consumer builds.

A change to these traits is a change to every driver: cut a release, then
move each driver's pin in its own pull request. A trait or enum that grows
in a later release is a minor version bump while the crate is below 1.0;
the enums that are expected to grow (`Feature`, `DamageKind`,
`StructureRole`, `NodeKind`, `HealthState`, `ApiError`, `SalvageError`) are
`#[non_exhaustive]`, so a driver matches them with a wildcard arm.
