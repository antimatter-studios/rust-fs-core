### Added

- **The three driver API layers are shared traits, behind the `layers`
  feature.** `fs_core::layers` defines `VolumeAPI` and `OpenVolume` (probe,
  open, open for salvage, volume bytes, geometry, the superblock and its
  copies, health, and what the driver supports on the volume),
  `PrimitiveAPI` (the on-disk structures, read-only and damage-tolerant, the
  salvage verbs bound to the volume's source, and geometry hypotheses for a
  source whose superblock is gone) and `FileAPI` (directory listing,
  metadata, positioned reads, path lookup by text or by bytes, and copying
  on a healthy volume). Every rust-fs driver implements them, so a consumer
  reaches any format through one set of traits. `docs/three-layers.md` is
  the contract and says how drivers pin this crate: by version or git tag,
  with a local `[patch]` kept out of the published manifest.
- **The salvage adapter, its evidence types and plans moved here from
  rust-fs-exfat, extended where ext4 and EROFS did not fit.** An
  `ExtentWalk` holds file-ordered `WalkExtent`s with file offsets, an
  `uninitialized` flag and an `Encoding` for compressed runs, so holes,
  unwritten extents and compressed data keep their place;
  `ExtentWalk::plan()` rebuilds a file from its walk, and
  `SalvageAdapter::read_encoded` decodes a compressed run through
  `PlanStep::Encoded`. A tree node's parents are `ParentClaim`s carrying the
  claiming entry's name and state, so hard links and deleted entries are
  told apart. A plan without encoded steps keeps the digest rust-fs-exfat
  computed. `Feature`, `DamageKind`, `PlanStep` and `SalvageError` are
  `#[non_exhaustive]`.
- **A conformance helper any driver runs over its own images.**
  `layers::conformance` checks each layer's contract -- bounded reads,
  health against its findings, capability flags against refusals, scan
  counts and resumes, walks in file order and decodable, the layers agreeing
  about the same bytes, every file reading back its size by its byte path --
  and `layers::conformance::salvage` is the deeper salvage set from
  rust-fs-exfat.
