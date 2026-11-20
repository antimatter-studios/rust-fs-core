### Added

- **The three driver API layers are shared traits, behind the `layers`
  feature.** `fs_core::layers` defines `VolumeAPI` and `OpenVolume` (open,
  probe, volume bytes, geometry, the superblock and its copies, health, and
  what the driver supports on the volume), `PrimitiveAPI` (the
  on-disk structures, read-only and damage-tolerant, with the salvage verbs
  bound to the volume's source) and `FileAPI` (directory listing, metadata,
  positioned reads, path lookup and copying on a healthy volume). Every
  rust-fs driver implements them, so a consumer reaches any format through
  one set of traits. `docs/three-layers.md` is the contract and says how
  drivers pin this crate: by version or git tag, with a local `[patch]` kept
  out of the published manifest.
- **The salvage adapter, its evidence types and plans moved here from
  rust-fs-exfat.** `SalvageAdapter`, `Candidate`, `ExtentWalk`, `Tree`,
  `Damage`, `Plan`, `PlanReader` and the reports are unchanged in shape, and a
  plan's digest is the value rust-fs-exfat computed. Two changes from
  rust-fs-exfat's shape, found while fitting ext4: an `ExtentWalk` holds
  file-ordered `WalkExtent`s with file offsets and an `uninitialized` flag,
  so holes and unwritten extents survive (`ExtentWalk::plan()` rebuilds the
  data in place, `ExtentWalk::contiguous` builds a chain-shaped walk); and a
  tree node's parents are `ParentClaim`s carrying the claiming entry's name
  and state, so hard links and deleted entries are told apart. `Feature`, `DamageKind`
  and `SalvageError` are now `#[non_exhaustive]`, so later releases can add to
  them.
- **A conformance helper any driver runs over its own images.**
  `layers::conformance` checks each layer's contract -- bounded reads,
  capability flags against refusals, scan counts and resumes, the layers
  agreeing about the same bytes, every file reading back its size -- and
  `layers::conformance::salvage` is the deeper salvage set from rust-fs-exfat.
