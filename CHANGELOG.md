# Changelog

Notable changes to `rust-fs-core` (published as `am-fs-core` up to 0.2.24), newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.

Every other driver in this family depends on this crate, so a change here
reaches all of them.

## [Unreleased]

## [0.3.5] — 2026-10-06

### Fixed

- **The release notes' diff link compares with the previous release.** It
  took the next heading starting with a digit, so a CHANGELOG keeping history
  under `## 0.2.0-dev` linked to a tag that does not exist. The previous
  version is now the next plain `X.Y.Z`.

## [0.3.4] — 2026-10-06

### Changed

- **`release-notes` reads a section headed `## vX.Y.Z` or `## X.Y.Z`** as
  well as `## [X.Y.Z]`, and finds the previous version for the diff link the
  same way. Several repositories in the family use the `v` form, and one
  has a pre-push hook that requires it for a tag.

### Added

- **`release-notes --unattested`** ends the notes with a provenance line that
  does not claim an attestation, for a repository whose release workflow
  attaches files it does not attest.

## [0.3.3] — 2026-10-06

### Added

- **`tier.sh --refuse-ignored`** fails a passing tier whose libtest summaries
  report ignored tests (exit 66), the gate rust-fs-squashfs kept in its own
  copy. It combines with `--refuse-skips`.
- **`tier.sh TIER -- COMMAND`** takes the tier's budget from the caller's own
  `scripts/tier-budgets.txt` (`TIER LINES BYTES`, `#` comments), so a
  repository that kept one measured table inside its copy of the runner keeps
  the table as data and runs this file in place. A tier with no row is refused
  (exit 2).

### Changed

- **A failing tier keeps its own status.** `--refuse-skips` overwrote it with
  66; both gates now refuse only a run that otherwise passed.

## [0.3.2] — 2026-10-06

### Changed

- **The family's scripts run in place; no repository keeps a copy** (#212).
  Every script works on the repository it is run from (`FS_CORE_CALLER`,
  else the git top-level of the current directory), so a repository runs
  `bash ../rust-fs-core/scripts/NAME.sh` from the checkout beside it, at the
  version it pins, and a bump upgrades the scripts with nothing to recopy.
  `family-check` now refuses a committed `scripts/core.sh` or
  `scripts/tier.sh`, the two wrappers that existed only to find this crate,
  and any call through them. `scripts/core.sh` stays here for repositories
  still on an older version.
- **One tier runner for the family.** `scripts/tier.sh` logs in the caller's
  `tmp/logs/` and takes `--refuse-skips`, the check three repositories had in
  their own copies, failing a run that printed `SKIP:` lines (exit 66).

### Fixed

- **`agents-core-check` checks the caller's AGENTS.md.** It found the guide
  beside itself, so run in place from another repository it checked this
  crate's guide and passed whatever the caller's said.

## [0.3.1] — 2026-10-06

### Added

- **`scripts/core.sh release-notes VERSION`: a release's notes are its
  CHANGELOG section** (#209). It prints the `## [VERSION]` section, a link
  to the diff from the previous version and the provenance line, and refuses
  a version the CHANGELOG does not describe. `release.yml` runs it before
  `cargo publish`, so such a tag stops before anything is uploaded, and uses
  it as the GitHub release's body; the shared `release-cli.yml` does the same
  before building a tarball. Releases used to say only "See CHANGELOG.md".
- **`scripts/core.sh changelog-draft [--write]`: a draft entry from the
  commits since the newest tag.** Squash subjects become Added (`feat:`),
  Fixed (`fix:`) and Changed bullets, each linking its pull request, release
  commits skipped; `--write` puts them under `## [Unreleased]`. A starting
  point: the explanation of a breaking or visible change is still written by
  hand.

## [0.3.0] — 2026-10-06

### Changed

- **Published as `rust-fs-core`, the repository's name.** The crate was
  `am-fs-core` up to 0.2.24, which stays on crates.io pointing here. A
  dependent changes one line, `am-fs-core = "0.2"` to `rust-fs-core = "0.3"`;
  the import (`fs_core`) and the C symbols are unchanged. The version moves
  to 0.3.0 so the new name starts on a version the old one never had.
- **`scripts/core.sh` finds core under either name** while the family moves,
  so a consumer still on `am-fs-core` and one already on `rust-fs-core` both
  resolve it.

## [0.2.24] — 2026-10-06

### Changed

- **The last version published as `am-fs-core`.** The crate is renamed to
  `rust-fs-core`, the repository's name; every later version is published
  under that name only. The code is 0.2.23's; only the description and the
  README change, to say where the crate went. The import (`fs_core`) and the
  C symbols are unchanged.

## [0.2.23] — 2026-10-03

### Added

- **`.github/workflows/release-cli.yml`: the one copy of the release
  tarball's package, attest and attach jobs** (#193, #202). Each repository
  that ships command-line tools carried its own `package-cli` matrix and
  attest-and-attach job in `release.yml`, and the copies drifted. This is a
  `workflow_call` workflow taking the caller's `core-ref` and `toolchain`: it
  builds on darwin-arm64 and linux-x86_64 with a read-only token, runs
  `scripts/core.sh package-cli` after a locked release build, and one job
  holding the write grants refuses anything but the two tarballs, attests
  them and attaches them to the caller's tag. A caller pins it by commit SHA
  and passes this release's tag as `core-ref`.
- `tests/release_cli_workflow.rs` reads the workflow as YAML and fails on
  each property a caller relies on, with one mutation per property.

## [0.2.22] — 2026-10-03

### Fixed

- **`family-check` refuses a `scripts/core.sh` that is not executable**
  (#200). It compared the caller's copy with `cmp`, which reads bytes and not
  the mode, so a copy that lost its executable bit in a conflict resolution
  passed and then failed with `Permission denied` where a guest suite ran it
  directly. Where git tracks the file, the mode git records decides, because
  the mode on disk depends on `core.fileMode`; otherwise the disk's does.

## [0.2.21] — 2026-10-03

### Fixed

- **`ci-gate` fails an aggregate whose steps never read a result it needs**
  (#198). It held `ci-ok`'s `needs:` to every job, but not what the step did
  with them, and three repositories had a step that judged a hand-kept list
  leaving out `semver`: a failing semver job left `ci-ok` green and the gate
  passed. The steps must now read `toJSON(needs)` or `needs.*.result` as a
  whole, or each need's own `needs.<job>.result`; a need none of them reads
  is named. A script handed `toJSON(needs)` is trusted with it -- the gate
  cannot see inside the script.

## [0.2.20] — 2026-10-03

### Added

- **`scripts/core.sh stage-siblings` and `scripts/core.sh guest-rust-run`:
  the one copy of the rest of a driver's test-VM Rust layer** (#195). Five
  Linux drivers carried the same staging in their `test:vm` task and the same
  head of `scripts/guest-suite.sh`, differing only in the filesystem's name.
  `stage-siblings SHARE SIBLING... [-- COMMAND...]` runs on the host: it
  stages each path sibling's HEAD on the VM share, replacing what was staged
  before, refuses a sibling that is not checked out or has no commit and a
  share that is not a directory, and then runs the guest command with
  chore's `CLI_ARGS` less `--verbose`/`-v`, unglobbed. `guest-rust-run NAME
  SHARE SIBLING... -- COMMAND...` runs in the guest: it refuses outside the
  VM, links each staged sibling beside the caller (pointing a stale link
  back, refusing a real directory in its place), exports the toolchain and
  build directories under `/var/lib/NAME-rust` and `/var/cache/NAME-target`,
  installs the pinned toolchain through `guest-rust-toolchain`, and runs the
  suite with its status. They answer `--version` with
  `rust-fs-core-stage-siblings 1` and `rust-fs-core-guest-rust-run 1`.
- `family-check` refuses a committed `scripts/stage-siblings.sh` or
  `scripts/guest-rust-run.sh`, and a workflow, `chores.yml` or script that
  runs one directly.

## [0.2.19] — 2026-10-02

### Added

- **`scripts/core.sh package-cli VERSION LABEL [TARGET-DIR]`: the one copy of
  the release tarball's packaging** (#193). Ten repositories each carried a
  `scripts/package-cli.sh` and a test of it, ten different files holding
  three policies for which names go in the tarball; one never moved to the
  multi-call layout and shipped a single tool for a whole release. The family
  script is the union of what the copies did -- the install-prefix layout,
  every dotted name a relative symlink to `bin/<repo>`, a page per name in its
  manual section and one per subcommand beside it, the zsh, bash and fish
  completions, `share/<repo>/CAVEATS` of at most four lines, the licences, and
  every name answering `--help` and `--version` as `<name> (<crate>)
  <version>` -- checked from the unpacked tarball, with no tarball left behind
  on any failure, not even a previous run's. What differs between
  repositories is read from the caller's `Cargo.toml` under
  `[package.metadata.package-cli]`: `names` (each dotted name and its man
  section), `licenses`, and optionally `caveats`. The names written there and
  the ones the binary lists must agree as sets, in any order, and every list
  is compared in the C locale -- a bare `sort` failed correct tarballs in
  eight repositories' tests this week. A page in a section no name has, a
  completion for a name not shipped, or any other file outside the layout is
  refused. It answers `--version` with `rust-fs-core-package-cli 1`.
- `family-check` refuses a committed `scripts/package-cli.sh`, and a
  workflow, `chores.yml` or script that runs one directly.
- `tests/package_cli.rs` runs it from caller trees against a stand-in binary
  that behaves, and one for each way a build can be wrong.

## [0.2.18] — 2026-10-02

### Added

- **`ci-gate` is a family script, served by `scripts/core.sh`.** Eleven
  repositories each committed a byte-identical `scripts/ci-gate.sh`, and the
  three that had none had nothing holding their required checks to `ci-ok`.
  It now runs against the CALLER's `ci.yml` and `.github-guard`
  (`FS_CORE_CALLER`), answers `--version` with `rust-fs-core-ci-gate 1`, and
  every repository runs it as `bash scripts/core.sh ci-gate`. The
  `CI_GATE_*` overrides are unchanged, and their paths are relative to the
  caller.
- `scripts/core.sh family-check` refuses a committed `scripts/ci-gate.sh` and
  a workflow, `chores.yml` or script that runs one, as it already did for
  `test-floor` and `semver-check`.

### Fixed

- **`.github-guard` is read as github-guard reads it**, with `git config
  --get-all checks.required`: one check per `required =` line, kept whole. The
  gate split each value on whitespace, so `full test suite (fixtures +
  integration)` was reported as six checks that do not exist, a trailing
  `; comment` git ignores was read as more checks, and a `required =` under
  some other section counted. A guard git cannot parse now fails, by name.

## [0.2.17] — 2026-10-02

### Added

- **`scripts/core.sh guest-rust-toolchain`: the one copy of the toolchain
  install a driver's test VM runs** (#190). Five Linux drivers each installed
  the guest's Rust toolchain in their own `scripts/vm-setup.sh`, and only one
  of the five recovered from an install a reaper or deadline interrupted; on
  the other four that wreckage, on a disk that outlives `vm:down`, broke every
  later `chore test:vm` until the VM was destroyed. The family script installs
  the channel the caller's `rust-toolchain.toml` pins, with rustfmt and clippy
  on the minimal profile, into a `RUSTUP_HOME`/`CARGO_HOME` the caller must
  name; clears rustup's download and tmp caches first; removes a toolchain
  that failed to install and tries once more, printing the first failure;
  fetches rustup again when the one on disk cannot answer `--version`; and
  holds a lock, because every worktree of a project shares one guest. It
  answers `--version` with `rust-fs-core-guest-rust-toolchain 1`.
- `family-check` refuses a committed `scripts/guest-rust-toolchain.sh`, and a
  workflow, `chores.yml` or script that runs one directly.
- `tests/scripts/test-guest-rust-toolchain.sh` drives the install through
  each kind of wreckage against a stub rustup that refuses the way the real
  one was measured refusing.

## [0.2.16] — 2026-10-02

### Added

- **The family scripts are this crate's, and every repository runs them from
  here.** `scripts/test-floor.sh` and `scripts/semver-check.sh` now run
  against the CALLING repository -- its `tmp/logs/`, its `Cargo.toml` -- and
  answer `--version` (`rust-fs-core-test-floor 1`,
  `rust-fs-core-semver-check 1`). `scripts/core.sh` is the one file a
  consumer carries: it finds this crate and runs the script named. The floor
  is the union of what the consumers' copies did: it survives a log with no
  result line, counts cargo-semver-checks' lints with colour stripped, and
  with `--refuse-ignored` fails a tier that ignored a test. `--targets FILE
  TIER` checks a floor per integration target, so a suite that empties from
  the inside cannot hide inside a healthy total (rust-fs-btrfs's per-target
  floors, now everyone's).
- **`scripts/core.sh family-check`** fails a repository that commits a copy of
  a family script, carries a `scripts/core.sh` that differs from this one, or
  runs a local `scripts/test-floor.sh` / `scripts/semver-check.sh` from a
  workflow, `chores.yml` or script. Every repository runs it in CI, so a copy
  cannot come back unnoticed.
- `tests/family_scripts.rs` runs every one from a caller tree, the way
  consumers do, and proves the bootstrap refuses a missing core and a script
  with the wrong `--version`.

## [0.2.15] — 2026-09-30

### Fixed

- **`generate man|completions` takes its SHARE directory as a path.** A
  directory whose name is not UTF-8 now reaches the filesystem instead of
  being refused by clap as a usage error with status 2. One driver's copy of
  the plumbing had gained this fix after the copies were reconciled into
  `fs_core::cli`, so it is carried here before that copy is deleted.

- **`doctor` asks a busy program again instead of calling it foreign.**
  Linux refuses to run a file something still holds open for writing
  (ETXTBSY) — an install still copying it into place, or any process that
  forked while it had the file open — and `doctor` took that refusal for a
  program that is not ours. It now retries for up to five seconds. The
  release run of v0.2.14 failed on exactly this in its own test suite.

## [0.2.14] — 2026-09-30

### Added

- **`fs_core::cli`, behind a new `cli` feature: the command-line plumbing the
  family's tools share** (#177). Dispatch on `argv[0]`, the `<tool> (<crate>)
  <version>` line, `<repo> doctor`, JSON results with `--text` as the opt-out,
  the structured error on stderr and its exit statuses, and `generate
  names|man|completions`. It was a directory every tool carried by hand
  (`src/cli/common/`), and the copies had drifted; this is their superset,
  with each drift settled once:
  - `doctor` reads a program's `--version` answer while the program runs.
    Most copies read it only after the program exited, so a program printing
    more than a pipe holds blocked, ran out the probe's five seconds and was
    reported as somebody else's.
  - The entry point's `--help` examples are aligned on the longest command.
    Most copies used fixed spacing, which misaligned as soon as a verb was
    longer than `mkfs`.
  - `--json`/`--text`: the one given last on the command line wins, as the
    contract always said. With the switches global, clap copies each level's
    switch into the other, so every copy answered text for `--text … --json`
    across a subcommand. A `--text` after `--` is data, not a switch.
  - `generate man|completions` is part of every tool, not only some.
  - A new `cli::respond` returns what would be printed and the status
    without printing it, so the contract is tested in-process.

  clap, clap_complete and clap_mangen are optional and reached only through
  the feature: the default build, and the static library, gain nothing
  (`tests/cli_feature_is_opt_in.rs` refuses otherwise).

- Releases carry a build-provenance attestation: the published `.crate` is
  attached to the GitHub release for its tag, checked first against the
  crates.io checksum, and verifiable with `gh attestation verify` (see the
  README, "Verifying a release").

### Changed

- **An over-budget verdict says where the rule is.** Exit 65 is a status a
  reader meets having read nothing else, and "raise the measured budget
  deliberately" does not say what a deliberate raise has to carry. The verdict
  now names `docs/output-budget.md`, which gained the section that answers it:
  a budget is a measurement, so a raise carries the measurement that justifies
  it, and neither silencing the run nor bypassing the wrapper is a way to meet
  one. The two older copies of this script ended the same verdict with a
  pointer of their own; core dropped it when it became the canonical copy
  (#153).

### Fixed

- **A verdict that could not be written is no longer reported as a failing
  tier.** `scripts/output-budget.sh` ended with the verdict `printf`, so that
  `printf`'s status became the script's: with stdout closed or full, a passing,
  in-budget run exited 1 with nothing in the log to explain it —
  `printf: write error: Bad file descriptor`, measured. This script's status is
  a claim about the command it wrapped, so it now exits 0 explicitly. The write
  error still reaches stderr; it has stopped being attributed to the tests
  (#153).

- **`docs/output-budget.md` describes the family as it is.** It still named the
  vendored copies #153 has since deleted, a `chore ci:gate` task that was
  reverted in favour of this repository's own `scripts/ci-gate.sh`, a
  `rust-fs-ntfs/scripts/resolve-output-budget.sh` that no longer exists, and
  the guest-transfer blocker that #153 itself retracted. Measured from every
  repository's default branch: this is now the only one that commits the
  script.

## [0.2.13] — 2026-09-26

### Changed

- **A failing tier says where its log is instead of reading it aloud.** The
  wrapper printed forty lines of tail on every failure. That is right for a
  person at a terminal and wrong for the reader who pays most: an agent
  re-reads its whole transcript on every later step, so those lines are paid
  for many times over — and they are rarely the forty that matter, because the
  assertion is usually further up the log than its last page.

  A failure now prints the verdict, the exit status, the log's path and its
  line count. `--tail N`, or `OUTPUT_BUDGET_FAIL_TAIL=N`, brings the old
  behaviour back for whoever is watching. A verbose run prints no tail at all,
  because it already streamed the run. This matches `fs-linux-test-harness`
  4b8e91b, which made the same change to the copy this script came from.

- **A superseded `FLTH_*` variable is reported rather than ignored.** The
  wrapper's variables became `OUTPUT_BUDGET_VERBOSE` and
  `OUTPUT_BUDGET_FAIL_TAIL` when it moved here from the harness. That rename
  fails silently — the old name is simply not read, and the run stays quiet —
  so setting `FLTH_VERBOSE` or `FLTH_FAIL_TAIL` now prints which name replaced
  it. It is not honoured: a fallback would keep the old name alive in habits
  and documentation indefinitely.

  Consumers pinning this script by SHA-256 (`rust-fs-ntfs`'s
  `scripts/resolve-output-budget.sh`) must move their digest with this change.
  `--version` is unchanged at `rust-fs-core-output-budget 1`: the flags, the
  exit statuses and the verdict on a passing run are all the same.

## [0.2.12] — 2026-09-22

`v0.2.11` has a section of its own again, below. It was tagged without one,
and the entries were folded in here; which side of the tag each fell on is not
a guess, because `git log v0.2.10..v0.2.11` says exactly — so they have been
moved rather than left (#162).

### Added

- **`BlockDevice::set_len` and `BlockDevice::can_grow`: a device can be asked
  to change its own length.** #75 refused a write past the end of a
  `FileDevice`, and it was right to — `write_all` at a seeked offset extends a
  file, so the backing store grew while `size_bytes` went on reporting its
  construction-time length, and a caller bounding its reads by `size_bytes`
  (which is what `CachingDevice` does, clamping every block it fetches) could
  never reach the bytes it had just written (#70).

  What it did not do is leave anything in its place. Appending a block,
  cluster or grain and recording where it went is how all four image formats
  in this family allocate, and a write past the end was the only tool any of
  them had for the first half. Measured on this machine, each crate on its own
  unmodified `main`, `../rust-fs-core` swapped between the two refs:

  | crate | against core `main` | against `v0.2.10` |
  |---|---|---|
  | `rust-img-vhd` | 253 passed, **8 failed** | 261 passed, 0 failed |
  | `rust-img-qcow2` | 252 passed, **3 failed** | 255 passed, 0 failed |
  | `rust-img-vhdx` | 264 passed, **8 failed** | 272 passed, 0 failed |
  | `rust-img-vmdk` | 245 passed, **12 failed** | 257 passed, 0 failed |

  Every failure is one write landing exactly at the device's current end, e.g.
  `OutOfBounds { offset: 20480, len: 4096, size: 20480 }`. (#147 counted 7/3/1/1
  against an older `main`; the crates have moved since.) See #147 and #129.

  **NOT A BREAKING CHANGE.** Both methods are defaulted — `set_len` to
  `Err(Error::ReadOnly)` and `can_grow` to `false` — so every existing
  implementor in every sibling crate keeps compiling and keeps behaving
  exactly as it did. A device that cannot change its length does nothing at
  all to adopt this.

  **On `BlockDevice` rather than a separate `GrowableDevice` trait**, because
  of what the consumers hold: `Arc<dyn BlockDevice>`, at 15 sites in vhdx, 7
  in qcow2, 6 in vhd and 5 in vmdk. A `dyn` type cannot be bounded onto a
  second trait without downcasting through `Any`, which turns "this device
  cannot grow" into a failed downcast at all 33 of them. It is also the idiom
  this trait already uses for an optional capability: `write_at` defaults to
  `Err(ReadOnly)` and `is_writable` to `false`.

  **The contract is the atomicity, not the signature.** An implementation must
  leave the backing store, the number `size_bytes` reports and any cached view
  of the device agreeing when it returns. One that extends the file and leaves
  `size_bytes` stale is #70 under a new name, and it passes a naive test —
  the write succeeds and only a later cached read finds the hole. Measured
  with `CachingDevice::set_len` forwarding to the device and sweeping nothing:
  a 6000-byte file behind a 4096-byte cache, the short last block warmed,
  `set_len(8192)`, then one read across the old end returns
  `ShortRead { offset: 5000, want: 3000, got: 1000 }`.

  Implemented on:

  - **`FileDevice`**, for a handle opened read-write on a regular file.
    `can_grow` is false for a read-only handle and false for a **block device
    node**, whose length belongs to the kernel — `ftruncate` there is not a
    resize, and promising an image writer room it can never get would be
    worse than refusing. `size` is an `AtomicU64` now; `size_bytes` still
    takes no lock, and `set_len` publishes the NARROWER of the two lengths
    first so the declared size never exceeds the file's real length at any
    instant.
  - **`CachingDevice`**, which sweeps the entries its new length makes short,
    once either side of the device call, for the same reason and in the same
    shape as `write_at`. `size_bytes` already forwarded, so the number
    follows for free; the entries do not.
  - **The `Arc<T>` and `Box<T>` forwarding impls**, without which
    `Arc<dyn BlockDevice>` would answer the refusing default over a device
    that can grow — method resolution finds those impls before it derefs —
    and the whole change would have done nothing for the four crates it
    exists for.

  Everything else keeps the refusing default, deliberately and not as an
  oversight: `ReadOnlyDevice`, `CallbackDevice`, `OwnedSlice` and
  `OwnedRwSlice`. A slice's length is the window it was cut to.

  `BlockRead::size_bytes`'s contract is amended to match: the number still
  never follows its backing store, and `set_len` is now the one door through
  which a caller can move it. (#147, #129)

- **One check gates a merge, and it stands for every job.** `ci.yml` grows an
  always-run `ci-ok` job that `needs:` every other job in the workflow and
  fails when any of them failed, was cancelled or was *skipped*, and
  `.github-guard` now requires that one name instead of the five real check
  names it used to list. A renamed job, an added matrix leg or a job split in
  two no longer changes what gates a merge without anyone deciding to, and a
  required check that no job produces — which GitHub reads as permanently
  pending, with nothing to point at — can no longer be left behind by a
  rename. Every driver in this family depends on this crate, so a job that
  quietly stopped gating here reaches all of them. `scripts/ci-gate.sh` holds
  both halves to it, run by `chore check:ci-gate` and by CI directly.

### Changed

- **The CI gate is a `chore` task, not a crate, and not a test.** #157 put the
  aggregate-gate rules in `crates/am-ci-guard`, taken here as a
  `[dev-dependencies]` entry and called from `tests/ci_aggregate_gate.rs`. It
  worked and it was well tested, and it was still the wrong container twice
  over: the rules exercise nothing this crate ships — they parse a YAML file
  and compare strings, sitting beside tests that read superblocks and walk
  extent trees — and the `test` job enforces an executed-test floor, so a
  meta-test inflates the very count this repository uses to satisfy its own
  gate. Being a crate also dragged a pure CI concern into the cargo dependency
  graph: crates.io publishing, version pins, and entanglement with #147, none
  of which has anything to do with checking that a YAML file agrees with a
  config file.

  So #157 is reverted. This is a single-package repository again —
  `am-fs-core`'s name, version, `fs_core` lib, `staticlib`/`rlib`,
  `path = "../rust-fs-core"`, dependency list and 65-file `cargo package
  --list` are all exactly what they were before the workspace conversion.

  The rules briefly moved into `antimatter-studios/chore` as a `ci:gate`
  subcommand, and that was wrong for a third reason: `chore` is a
  general-purpose task runner this project merely consumes, and putting them
  there made cutting a `chore` release a prerequisite for a change here. That
  was reverted too, and `chore`'s history is as it was.

  They are now `scripts/ci-gate.sh`, run by a `chore check:ci-gate` task that
  names the script and nothing else — so the script is what can be tested,
  reviewed and run without `chore` at all — and by CI directly, since CI here
  does not install `chore`. Same four checks and the same two-way non-gating
  rule. It reads the workflow as YAML rather than scanning lines: a quoted
  key, a flow mapping and a `run: |` block whose contents look like a job key
  are all ordinary YAML a line scan reads wrongly.

  **`.github/actions/install-chore` stays.** It is
  antimatter-studios/chore#52 and it was never part of what was wrong here.

- `SliceGeometry::rebase`'s doc comment no longer claims a check against
  the parent that it never performed. It said it returned `None` "when
  the rebased offset would not fit on the parent at all"; there was no
  parent in scope, and the only thing tested was that the address fit in
  a `u64`. The C header made the matching promise, that the slice's
  addressable range "is `length` bytes", and now states the clamp.

## [0.2.11] — 2026-09-21

Reconstructed after the fact: the tag was cut without a section, and #162
tracked putting one back. The entries below are the ones `v0.2.10..v0.2.11`
carries — 52 commits — separated from `0.2.12`'s seven by the tag itself
rather than by recollection.

### Added

- **The test-output budget is now a packaged, canonical family asset.**
  `scripts/output-budget.sh` keeps passing test runs quiet while retaining
  complete logs, and `scripts/test-floor.sh` rejects a green run that did not
  execute the expected suite. The behavior is covered by tests, CI retains
  the logs and coverage report, and consumers can resolve the script from the
  published crate without cloning this repository.

- **The geometry arithmetic is fuzzed, on two tiers.** Nothing here
  parses a filesystem, so there is no structure to mutate: what this
  crate does is arithmetic on offsets and lengths that ultimately came
  from an image, in a release profile with `overflow-checks` off. So the
  input is read as *geometry* and each target asserts a **property**
  rather than merely surviving — a slice answers a read inside it with
  the parent's bytes from `start + offset`, `SliceReader` and
  `OwnedSlice` agree, and a cache answers exactly what the device under
  it would, including on the second read of the same range.

  That distinction is the point. "Arithmetic that wrapped in the release
  profile and answered a read inside a slice's own declared length with
  somebody else's bytes" is on the list of things the 2026-09-06 wave
  fixed by hand, and a target that only checked for panics would sail
  straight past it. Verified by injecting an off-by-one into `rebase`
  and a widened bound: both are caught as *wrong answers*, not crashes.

  The parent device is filled with a position hash rather than zeros,
  because a slice reading from the wrong offset returns bytes that are
  perfectly valid and belong somewhere else — only content that differs
  per position can tell the two apart, and
  `the_parent_pattern_distinguishes_every_offset` checks it actually
  does (#146).

### Fixed


- The release workflow now runs the suite under `--release` as well as
  debug, so the profile that gets published is tested (#111). It also
  sets `AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP`, as `ci.yml` does. Without it
  the next tag's test job would have failed in
  `tests/device_node_size.rs`, which was added after v0.2.10.
- `CachingDevice::stats()` and the constructors' `capacity` are
  documented. `stats()` is `(hits, misses)` over block lookups inside the
  cache: a read the cache bypasses (past the end, or spanning more than
  half the cache) moves neither, so `misses` is a lower bound on device
  reads (#123). `capacity` counts blocks, and `0` still caches one block
  rather than disabling the cache; doctests pin both (#124).
- A slice can no longer report more device than its parent holds. All
  three constructors — `SliceReader::new`, `OwnedSlice::new`,
  `OwnedRwSlice::new` — now ask the parent its size and clamp `length`
  to what is actually there, so `size_bytes()` is a fact rather than the
  caller's claim. It was stored verbatim and reported straight back, so
  `OwnedSlice::new(hundred_byte_device, 0, 1_000_000)` was a device that
  told every consumer it held a megabyte — and `size_bytes()` is the
  number the bounds checks in this crate, and the structures of every
  driver stacked on a slice, are sized from.

  Two things went wrong downstream of that lie, both now pinned by
  tests. A read inside the declared length but past the parent's real
  end was forwarded, so the caller got the parent's `ShortRead` with the
  parent's absolute offset and a non-zero `got`, with the readable
  prefix already copied into its buffer — the opposite of the "refuses
  before it touches the parent, `got: 0`, buffer untouched" contract the
  module documents. And a write past that end was forwarded too:
  `FileDevice::write_at` seeks and writes with no bounds check of its
  own, so an over-long RW slice over an image file returned `Ok` and
  EXTENDED THE FILE, 64 bytes becoming 72 in the test that now catches
  it.

  The length is clamped rather than the slice refused, because a
  partition table is bytes off the disk: a `dd` of the first part of a
  disk, or a table left stale after a shrink, both produce a last
  partition that runs off the end, and refusing it takes away the one
  thing someone with a truncated image wants. The rule is
  `slice::window_on_parent`, and `am-partitions` — which had to write
  this rule for itself, at the call site, because the constructor would
  not enforce it — can now use it instead of its own copy.

  A window whose `start` is at or past the parent's end has nothing
  behind it. The Rust constructors are infallible and make it a
  zero-byte slice; `fs_core_device_slice_ro` / `_rw` return NULL with a
  message saying which start and which parent size, because a zero-byte
  handle is honest and useless to debug — the mount that follows fails
  on its superblock read with no hint that the window was the problem.

  This closes the `start + offset + len` overflow in
  `SliceGeometry::rebase` at the root rather than with another
  `checked_add`: that sum can only leave a `u64` if `start + length`
  does, and `length` is now at most `parent_size - start`.

## [0.2.10] — 2026-09-06

### Changed

- `FileDevice` reads are positioned, and no longer take a lock. A read
  was `seek` then `read` under one mutex, so the file's cursor was
  shared state and two threads reading different offsets took turns —
  not because the device could not serve them at once, but because one
  would have moved the other's cursor. Unix uses `pread`, which takes
  the offset as an argument, so readers genuinely overlap. Windows keeps
  the lock, because its `seek_read` does move the file pointer. Writes
  keep it on both, since a write is still seek-then-write and a partial
  one must not have another writer's seek land inside it.

## [0.2.9] — 2026-09-06

### Fixed

- `CachingDevice` reads the last block of a device without running off
  the end. It always fetched a whole block, so a device whose length is
  not a multiple of the block size failed on its final block — and a
  device shorter than one block failed on the very first read. That is
  not a corner case: `am-fs-squashfs` takes its block size from the
  archive superblock, usually 128 KiB, and a small image is a few
  kilobytes whole, so caching one failed immediately. Short blocks are
  now fetched at their real length and cached that way. A read running
  past the end of the device is still an error, handed to the device to
  refuse, rather than being served short with no error.

## [0.2.8] — 2026-09-06

### Changed

- `CachingDevice` serves a read from the blocks it falls in, whatever
  its size or alignment. It cached only a read that was exactly one
  aligned block and passed everything else straight through, which is
  almost every metadata read a driver makes: measured against an XFS
  image, the average read was 1040 bytes against a 4096-byte block, so
  the cache was bypassed by the traffic it exists for. A read spanning
  more than half the cache still passes through, because caching it
  would evict everything to hold bytes the caller already has.
- A cache of one block no longer declines every read. The pass-through
  rule was "spans more than half the capacity", which one block against
  a one-block cache always does, so the smallest cache anybody could
  ask for silently did nothing.

## [0.2.7] — 2026-09-06

### Added

- `CountingDevice` — one instrument for measuring what a driver asks of
  its device. Wraps a `BlockRead`, counts calls and the bytes they
  asked for, and can be reset so a mount's own reads are not charged to
  the operation being measured. Both counters matter: a change that
  halves reads and doubles bytes is a readahead that guessed wrong, and
  one number alone would call it a win.
- `CachingDevice::read_only` — the cache can wrap a device that is only
  read. It required an `Arc<dyn BlockDevice>`, and every driver in this
  family mounts through an `Arc<dyn BlockRead>`, so a read-only mount
  could not use it at all: four of the six drivers cached nothing, not
  by choice but because it was not expressible. A write to such a cache
  is `Error::ReadOnly`; a flush succeeds, because nothing was written.

## [0.2.6] — 2026-09-06

### Added

- `ffi::panic_message` is public. Every other crate in the family guards
  its C entry points with its own `catch_unwind` and, having no way to
  reach this, reported a panic as `"panic in <function>"` — the name of
  the function that was running, which the caller already knew, in place
  of the message, which is the only part it did not. The guards
  themselves stay private to each crate: each records into its own
  thread-local, which is what its own C callers read.

## [0.2.5] — 2026-09-06

### Fixed

- A slice no longer rebases a read off the end of its parent. Turning an
  offset inside a slice into an offset on the parent was deliberately
  unchecked, on the argument that a slice built with a nonsense start
  would overflow there rather than quietly read elsewhere — but
  overflow-checks is off in the release profile these crates ship, so
  the addition wrapped and did exactly what the argument said it
  avoided. A slice starting at 2^63 and declared 2^63 + 51200 bytes
  long, which is what a GPT entry of `starting_lba` 2^54 and
  `ending_lba` 2^55 + 99 produces, answered a read inside its own
  declared length with `Ok` and the parent's bytes from offset 5000. A
  slice's geometry comes off the disk, so a start and a length that add
  past 2^64 are an ordinary thing to be handed.

## [0.2.4] — 2026-09-04

### Changed

- **One callback convention, and one test device.** The C callback surface had
  drifted into more than one convention for the same idea; it is now stated
  once. Four separate in-test block devices — which had quietly diverged on
  what a short read means — collapse into one, so a driver's tests and this
  crate's tests now agree about the device they are testing against.
- **Bounds errors say which bound was crossed.** A caller who overran a device
  got an error that did not distinguish "past the end" from "not aligned",
  which is the difference between a bug in the caller and a corrupt image.

### Fixed

- **An FFI guard that keeps the panic message.** The `catch_unwind` boundary
  turned every panic into a bare error code, discarding the message. A panic
  crossing an FFI boundary is already the worst case to debug; losing what it
  said made it worse.

## [0.2.3] — 2026-08-29

### Added

- Push/PR CI with a coverage gate, and the lint gate can now be run locally —
  the same one CI runs, so a green local run means something.
- `Cargo.lock` is committed and the gate commands pass `--locked`, with a
  pre-commit hook that refuses unpinned or stale dependencies. A release built
  from a floating dependency is not reproducible.

## [0.2.2] — 2026-06-09

### Changed

- Pinned toolchain moves from 1.94.1 to 1.95.0. Every crate in this family
  moves its `rust-toolchain.toml` in lockstep; a straggler links two copies of
  `_rust_eh_personality` into any consumer that binds both.

### Added

- Coverage for the pure functions that had none.

## [0.2.1] — 2026-05-12

### Changed

- CI actions bumped to node24-capable versions.

## [0.2.0] — 2026-05-12

### Added

- **`BlockReadStreamer`**, plus a `Read`/`Seek` adapter over `BlockRead`, so a
  consumer that wants a stream no longer has to write its own cursor over the
  block interface.
- Integration coverage across the public API.
- Release-on-tag pipeline using trusted publishing.

## [0.1.0] — 2026-05-10

### Added

- Initial release: the block-device abstraction the filesystem drivers share.
- `fs_core_device_from_callbacks`, so a host that owns the I/O (an FSKit
  extension, a WinFSP driver) can hand this crate a device without this crate
  knowing how the bytes are fetched.
- `OwnedRwSlice` and the `fs_core_device_slice_ro` / `_rw` C ABI, for
  addressing a partition inside a whole-disk device.

[Unreleased]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.5...HEAD
[0.3.5]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.24...v0.3.0
[0.2.24]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.23...v0.2.24
[0.2.23]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.22...v0.2.23
[0.2.22]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.21...v0.2.22
[0.2.21]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.20...v0.2.21
[0.2.20]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.19...v0.2.20
[0.2.19]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.18...v0.2.19
[0.2.18]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.17...v0.2.18
[0.2.17]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.16...v0.2.17
[0.2.16]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.15...v0.2.16
[0.2.13]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.12...v0.2.13
[0.2.12]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.11...v0.2.12
[0.2.10]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.9...v0.2.10
[0.2.9]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.8...v0.2.9
[0.2.8]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.4...v0.2.5
[0.2.15]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.14...v0.2.15
[0.2.14]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.13...v0.2.14
[0.2.11]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.10...v0.2.11
[0.2.4]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/antimatter-studios/rust-fs-core/releases/tag/v0.1.0
