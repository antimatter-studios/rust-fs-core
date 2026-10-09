# Working in rust-fs-core (agent guide)

Pure-Rust block-device framework: the `BlockRead` / `BlockDevice` traits, the
device implementations every driver composes over them, and the C ABI they all
share. Twelve sibling crates depend on this one, so a change here is a change
to all of them. This file is the fast path for an agent picking up work, so the
workflow does not have to be re-derived each time. It points at the existing
docs rather than duplicating them:

- **README** → `## What it gives you`, `## Intended consumers`, `## The CI gate, and the chore installer`.
- **docs/output-budget.md** → the neutral wrapper this repository owns and ten siblings borrow.
- **`.github-guard`** → why one required check and not five, argued at length.

The section between the BEGIN/END markers below is **shared, byte-identical,
with every repository in this family**. Do not edit it here: change the
canonical copy and propagate it, or `chore lint` will fail. Everything after
the END marker is specific to this repository.

<!-- BEGIN SHARED BLOCK: agent-core v5 sha256:93dca03d900fede9964388123030f779939bc99b8b9943f092051be6c9134126 -->
## Claiming work

Several agents work these repositories at the same time. Before you start on
an issue, claim it, so nobody else spends a session on what you are already
doing. The lock is a **GitHub label**, because labels are shared state that
every agent can read and change without posting comments into the thread.

**Before starting.** Check, claim, then read back:

```sh
gh issue view <N> --json labels                      # holds `claimed`? pick another
gh issue edit <N> --add-label claimed --add-label claim/<session>
gh issue view <N> --json labels                      # read back and confirm
```

`<session>` is your session name — `agent-<random4>-<isodate>`, e.g.
`agent-3f7c-2026-09-22`. Create the `claim/<session>` label if it does not
exist.

**Resolving a race.** Adding a label is not compare-and-swap: two agents can
both add `claimed` and both believe they won. That is what the read-back is
for. If it shows more than one `claim/*` label, the **lexically lowest**
session keeps the issue; every other agent removes its own `claim/*` label and
picks different work. Each racer computes the same answer independently, so no
further coordination is needed.

**When you finish or stop.** Remove both labels — on merge, or the moment you
abandon the work:

```sh
gh issue edit <N> --remove-label claimed --remove-label claim/<session>
```

Delete your `claim/<session>` label from the repository at the end of your
session so they do not accumulate.

**Reclaiming a stale claim.** An agent that dies holding a claim would block an
issue forever. If `claimed` was applied more than 12 hours ago and the holder's
branch has no commits since, any agent may take it: remove the stale `claim/*`,
add your own, and say so in the issue.

**This is a convention, not a fence.** Nothing enforces it. An agent that
ignores it duplicates work; it cannot corrupt anything. Honour it anyway.

## Work in a worktree

Every working copy is a **git worktree** of an existing checkout, made with
`git worktree add`. Never `git clone` a second, unlinked copy — not for a
branch, a PR, a review, or a sibling you need at another ref:

```sh
git -C <checkout> fetch origin
git -C <checkout> worktree add <path> -b <type>/<name> origin/main   # new work
git -C <checkout> worktree add --detach <path> <tag>                 # a sibling at a pinned ref
git -C <checkout> worktree remove <path>                             # when done
```

A worktree shares the checkout's objects and remotes, and `git worktree list`
shows it to every agent on the machine, so nobody else mistakes it for
abandoned work or loses track of it. An unlinked clone copies all the history
again, is invisible to that list, and gets left behind in `/tmp` long after the
work that made it is merged. Remove your worktree when you finish.

## Skills to use

- **`dev-loop`** — the required loop for any non-trivial change: baseline the
  full suite → change → re-run (no baseline test may regress) → enhance tests →
  vet. Always run it.
- **`commit`** / **`pr`** — for grouping commits and opening pull requests.

Each repository names any further skills of its own below.

## A bug fix starts with a red

**Prove it is broken first** — a failing check or test — *then* fix it, *then*
prove that same check is green, *then* confirm the full baseline still passes.
Never write the fix before you have a red. A fix with no failing test to its
name is a claim, not a result.

**Red and green happen in one pull request, on one branch.** Push the commit
that adds the failing test on its own, and let CI show it red on that pull
request. Then push the fix to the **same branch**, with the test untouched,
until the same pull request is green.

- Never put a fix in a second pull request, stacked or not. A pull request
  that only holds a red commit can never merge, and it blocks every pull
  request built on it.
- Never push the test and the fix together. The red has to be visible in CI,
  not claimed in the description.
- Wait for the red run to finish before pushing the fix: a new push cancels
  the run in progress.

## Nothing skips

A test that cannot run **fails**, naming the task that would provide what it
needed. Never add an early return for a missing fixture, tool or VM: a skipped
test reads exactly like a passing one, and a suite that quietly declines to run
is indistinguishable from a suite that passes.

Where a tier reports skips or ignored tests, that is a gate, not a note.

## Validate against something that is not us

A driver's own readers share its interpretation of the format, so they cannot
catch a misreading: the mistake is baked into the fixture *and* the parser, and
they agree with each other while disagreeing with every real filesystem. Unit
tests over self-built fixtures prove self-consistency, not correctness.

Every structure that is parsed or written gets a cross-validation test against
an **independent oracle** — the platform's own tools, a real kernel, or a third
implementation — before it is considered done. Each repository names its
oracles below.

## Output is budgeted

Test tiers run through `scripts/tier.sh`, which runs the suite **quietly**: the
whole run goes to `tmp/logs/<tier>.log`, a pass prints one verdict line naming
that log, and a failure prints the verdict, the command's status and the log's
path — `--tail N`, or `OUTPUT_BUDGET_FAIL_TAIL=N`, prints the tail for whoever
is watching. **Read the log**: a failing tier names it and does not recite it.
CI keeps the logs as an artifact, so the detail is always retrievable.

The budget caps the log, not merely what is shown, and every number in the
table was measured. A run that passes but prints more than its budget **fails**.

The reader who pays most for a noisy suite is an agent that re-reads its whole
transcript on every step, and so pays for one loud run many times over. If a
tier legitimately grows, raise its row **with the measurement that justifies
it**. Do not silence output to fit, and do not route around `tier.sh`.

## Commits and branches

- Branches are `<type>/<name>`, matching the commit type: `fix/`, `feat/`,
  `ci/`, `docs/`, `chore/`, `test/`.
- A commit is a subject plus flat one-sentence bullets. Subjects are
  declarative, not imperative: "the run-end bound is checked", not "check the
  run-end bound".
- **No AI attribution and no co-author trailers**, in commits or in pull
  request descriptions.
- `main` takes **squash merges only**.
- **Bring a branch up to date before every push.** `git fetch origin`, and
  if `main` has moved past the branch, rebase onto `origin/main` first,
  resolving any conflicts then, while they are small. Synced first, the run
  tests the branch close to how it will land. And a branch synced at every
  push never goes stale: each sync takes in only what landed since the last
  one, so conflicts stay few and small. Sync when you are pushing anyway; a
  push made only to bring a branch up to date buys a whole CI run and
  nothing else.
- **`main` merges through a merge queue.** A pull request whose own CI is
  green goes into the queue: `gh pr merge --squash` adds it. The queue tests
  each group on a temporary branch, `main` plus the queued pull requests in
  order, and squash-merges each one only when that run is green, so what
  lands is exactly the tree CI ran. That run is what "branches must be up to
  date" used to buy, at one run per group instead of one per pull request per
  merge: never update a branch only so that it can merge. Every workflow
  that gates `main` triggers on `merge_group`, or the queue never sees its
  result and nothing merges.

## Project rules

- **No GPL/LGPL/AGPL dependencies.** Permissive only (MIT/BSD/Apache).
  Shelling out to a copyleft CLI as a *test oracle* is fine — linking or
  copying it is not.
- **Each of these is a standalone project.** Never mention a consuming
  application in the README, the source, or CLI help.
<!-- END SHARED BLOCK: agent-core v5 -->

## Skills specific to this repository

None. The shared set is the whole set: `dev-loop` for any non-trivial change,
`commit` and `pr` for landing it. There is no `.claude/skills/` directory here,
and a skill that would be useful to every sibling belongs in agent-skills
rather than in one consumer of it.

## Running tests

`chores.yml` is the interface, and each task is one thing CI also runs:

```sh
chore lint            # cargo fmt --check, the agent-core check, clippy --locked -D warnings
chore build           # debug build
chore test:debug      # the whole suite, debug, overflow checks armed, budgeted, floor 330
chore test:release    # the same selection under --release, budgeted, floor 330
chore test            # both tiers
chore test:scripts    # the shell tests (tests/scripts/*.sh)
chore coverage        # the suite under llvm-cov with the same 90% line floor CI uses
chore staticlib       # libfs_core.a + include/fs_core.h into dist/
chore artifact        # print the absolute path of dist/ — a DIRECTORY, not a file
```

Two environment variables are part of the contract rather than conveniences:

- **`EXPECT_OVERFLOW_CHECKS=1`** tells the runtime probe in `src/lib.rs` that
  this is the run which must trap an overflow. `overflow-checks` is on in debug
  and off in release, so a defect whose only symptom is an arithmetic panic
  cannot be seen by a release-only run. `tests/ci_profile.rs` fails if no
  `cargo test` in `ci.yml` runs without `--release` while setting it — so the
  debug tier cannot be tidied away as a duplicate, nor given `--release` for
  consistency with a sibling.
- **`AM_FS_CORE_ALLOW_UNPRIVILEGED_SKIP=1`** acknowledges a real gap rather
  than papering over one. `tests/device_node_size.rs` needs a loop device,
  `losetup` needs root, and the runner is not root; without the variable that
  test **fails** rather than skipping, because libtest discards a passing
  test's output and a printed "skipped" line reaches nobody.
  `tests/ci_acknowledges_the_privilege_gap.rs` parses `ci.yml` and fails if any
  step that runs the suite omits it — which is not hypothetical: `coverage`
  runs the suite a second time under llvm-cov and failed there while `test`
  passed on the same commit. Do not set it to make an unrelated failure go
  away, and do not add it to a job that does not run the suite.

## The oracle here is the operating system

This crate parses no on-disk format, so there is no third-party reader to
disagree with. What it can get wrong is what a real device does, and the oracle
for that is the kernel itself: `tests/device_node_size.rs` builds a loop device
and requires the `BLKGETSIZE64` probe to report the size the OS reports, and
macOS covers its own equivalent in full, unprivileged, through `hdiutil`. The
probe's failure path goes through `/dev/zero`, needs no privileges, and runs on
every runner.

Two more independent graders stand behind that, and neither is this crate
marking its own homework:

- `cargo llvm-cov --fail-under-lines 90` in the `coverage` job.
- An **executed-test floor of 330** on every tier (`scripts/core.sh test-floor`). A
  suite that silently stops running a material part of itself otherwise reports
  exactly the same green as one that passes.

## How the budget is wired here — this repository owns the wrapper

`scripts/output-budget.sh` is the canonical, family-neutral copy. Ten siblings
resolve it from their pinned `../rust-fs-core` checkout and validate it by
SHA-256 and API version; none of them may carry its own copy, and none of them
may edit this one to suit itself. `docs/output-budget.md` is the contract.
`#153` is the open work on the copies that diverged before that was true.

Locally it is reached through `scripts/tier.sh LABEL LOG MAX-LINES MAX-BYTES
-- CMD...`, which runs a tier quietly into `tmp/logs/<LOG>.log`, printing one
verdict line on success and the tail on failure.

## The family scripts — one copy, here, run by every repository

`scripts/test-floor.sh`, `scripts/semver-check.sh`, `scripts/ci-gate.sh`,
`scripts/guest-rust-toolchain.sh` and `scripts/package-cli.sh` are the family's, the same way
`scripts/output-budget.sh` is. Each repository once carried its own copy of
them, and the copies drifted: one silent-exit bug in the floor was
fixed seven times, and a rule one repository added reached no other. Now no
repository carries either. Each carries `scripts/core.sh`, byte-identical to
this repository's, which finds rust-fs-core (`FS_CORE_ROOT`, the sibling
checkout, or cargo's rust-fs-core), insists on the script's `--version`, and
runs it against the calling repository:

- `scripts/core.sh test-floor [--refuse-ignored] TIER FLOOR` reads the
  caller's `tmp/logs/TIER.log` and fails when fewer than FLOOR tests -- or a
  semver run's lints -- executed, or with `--refuse-ignored`, when any test
  was ignored.
- `scripts/core.sh semver-check` runs cargo-semver-checks on the caller's
  crate against its newest crates.io release.
- `scripts/core.sh ci-gate` holds the caller's `ci.yml` and `.github-guard`
  to one required check: `ci-ok` needs every job, its steps read every result
  it needs (`toJSON(needs)` as a whole, or each `needs.<job>.result`), and the
  guard, read with `git config` as github-guard reads it, names `ci-ok` alone. It needs
  python3 with PyYAML, and git. A repository with no `Cargo.toml` (a Go one)
  has no cargo to find core through, so its CI checks out rust-fs-core
  beside it or sets `FS_CORE_ROOT`.
- **Documentation-only pull requests skip the heavy jobs.** A `changes` job
  pipes the pull request's paths to `scripts/code-changed.sh` (run in place
  from the rust-fs-core sibling), which prints `true` unless every path is
  documentation (`*.md`, `LICENSE*`, `.claude/**`), and exposes it as
  `outputs.code`; on a push or a tag it is always `true`. A heavy job whose
  only condition is `if: needs.changes.outputs.code == 'true'` (and which
  needs `changes`) may sit in `ci-ok`'s needs: the gate accepts it as long as
  `ci-ok` needs `changes` and reads `needs.changes.outputs.code`, so `ci-ok`
  can accept that job's skip when, and only when, `code` is `false`. Any
  other job-level condition is still refused. Lint, unit and semver stay
  unconditional: they are cheap, and lint is what checks a documentation
  change.

- `scripts/core.sh guest-rust-toolchain`, run inside a driver's test VM,
  installs the toolchain the caller's `rust-toolchain.toml` pins into
  `RUSTUP_HOME`/`CARGO_HOME` (both required), and recovers from the wreckage
  an interrupted install leaves on the guest's disk: a `.partial` download, a
  half-populated toolchain, a half-written rustup. It runs from the driver's
  `[test] guest_command` (`scripts/guest-suite.sh`), because that is the
  first point at which this repository is in the guest -- `test:vm` stages
  the pinned checkout on the share -- and the harness's `[setup]` script runs
  before it is. fs-linux-test-harness is not the home: it refuses by design
  to name a language or a toolchain. `tests/scripts/test-guest-rust-toolchain.sh`
  drives each kind of wreckage against a stub rustup (#190).

- `scripts/core.sh package-cli VERSION LABEL [TARGET-DIR]` builds the
  caller's release tarball in the current directory and prints its name: the
  install prefix every repository ships (the multi-call binary, a relative
  symlink per dotted name, a man page per name in its section, the zsh, bash
  and fish completions, `share/<repo>/CAVEATS`, the licences), checked from
  the unpacked tarball before it is named. What is the caller's own is read
  from its `Cargo.toml`, never written in the script:

  ```toml
  [package.metadata.package-cli]
  names = { "mkfs.example" = 8, "fs.example" = 1 }  # every dotted name, and its man section
  licenses = ["LICENSE"]                            # files at the repository's root
  caveats = "packaging/CAVEATS"                     # optional; this is the default
  ```

  The binary's `generate names` must list exactly those names. Ten
  repositories each carried a copy of this script, and of its test, until
  the copies held three policies for which names ship (#193);
  `tests/package_cli.rs` is the one test of the one script. It needs cargo
  and python3, and every list in it is compared in the C locale.

- `scripts/core.sh stage-siblings SHARE SIBLING... [-- COMMAND...]`, run on
  the host by a driver's `test:vm`, puts each path sibling checked out at
  `../SIBLING` on the VM share at `SHARE/siblings/SIBLING`, from its HEAD
  through `git archive`, replacing what was staged before; then runs
  COMMAND (the harness's `vm.sh guest-test`) with chore's `CLI_ARGS`
  appended, less the tier wrapper's `--verbose`/`-v`. It has to be a host
  script: it runs before the guest exists.

- `scripts/core.sh guest-rust-run NAME SHARE SIBLING... -- COMMAND...`, run
  by a driver's `scripts/guest-suite.sh` inside the VM, refuses unless
  `FLTH_GUEST=1`, links each staged sibling at `../SIBLING` (in the guest,
  `/SIBLING`), exports `RUSTUP_HOME`/`CARGO_HOME` under `/var/lib/NAME-rust`
  and `CARGO_TARGET_DIR=/var/cache/NAME-target`, installs the toolchain
  through `guest-rust-toolchain`, and runs COMMAND from the caller. The
  bootstrap cannot find core before core is linked, so the caller sets
  `FS_CORE_ROOT=/share/siblings/rust-fs-core` for this one call. It is not
  named `guest-suite`, because that is each driver's own guest command and
  family-check would take it for a copy. `tests/scripts/test-stage-siblings.sh`
  and `tests/scripts/test-guest-rust-run.sh` drive both (#195).

- `scripts/core.sh family-check` fails the caller if it commits a copy of
  any of these, if its `scripts/core.sh` differs from this repository's or
  is not executable (in the mode git records, where git tracks it), or if
  anything it runs calls a local copy. Every repository runs it in CI.

This repository runs its own through `scripts/core.sh` too, so the path every
consumer takes is the one tested here (`tests/family_scripts.rs`, and
`tests/scripts/test-ci-gate.sh` for the gate, which needs PyYAML, and
`tests/package_cli.rs` for the tarball). A change to
either script is a change for the whole family: keep the `--version` contract,
or bump it and every consumer's `core.sh` in the same pass.

| tier | measured | budget | floor |
|---|---|---|---|
| debug | 557 lines / 31,250 bytes | 750 / 50,000 | 330 |
| release | 557 lines / 31,295 bytes | 750 / 50,000 | 330 |
| coverage | 575 lines / 32,607 bytes | 800 / 60,000 | 330 |

Exit **65** is a passing run that printed more than its budget.
`AM_FS_CORE_VERBOSE=1`, or `chore test -- --verbose`, streams a run as well as
logging it and does **not** lift the budget. CI keeps `tmp/logs/` as an
artifact, so the detail is always retrievable.

One step in the `test` job asserts that `cargo package --locked --list` still
contains `scripts/output-budget.sh`. A consumer that cannot find the sibling
falls back to the packaged crate, so dropping that file from the package is a
silent break in ten repositories rather than a failure here.

## What gates a merge

**One required check: `ci-ok`.** It carries `if: always()`, `needs:` every
other job in `ci.yml` — `changes`, `test` (the ubuntu / macOS / Windows
matrix), `fmt`, `coverage` and `semver` — and fails when any of them failed,
was cancelled or was **skipped**, except that a documentation-only change
(`changes` says `code` is `false`) skips `test` and `coverage`, and then their
skip is accepted. It runs no tests of its own, deliberately: it is a claim
about the other jobs, so it must not be able to pass work of its own off as
theirs. `ci.yml` triggers on `merge_group`, so the merge queue sees it.

`.github-guard` requires that one name and nothing else, and github-guard reads
it **from the server copy of the default branch**, never from the working tree
— which is what stops a branch checkout from unprotecting `main`. That is also
what makes the aggregate safe: a pull request can rename, split or replace the
jobs behind `ci-ok` without protection ever noticing.

The two failure modes this shape exists to close, both of which have happened
in this constellation:

- A job added to `ci.yml` and not to `ci-ok`'s `needs:` reports on every pull
  request and gates nothing.
- A required context that no job produces reads to GitHub as permanently
  *pending*, not failing. With `enforce_admins` on, nothing merges and there is
  no red check to point at.

**`chore check:ci-gate` holds both halves mechanically** — every job in `ci.yml`
must appear in `ci-ok`'s `needs:`, and `.github-guard` must require `ci-ok` and
nothing else. The task runs `scripts/core.sh ci-gate` and nothing else, so the
script is what can be tested, reviewed and run without `chore` at all.

It took three attempts to put this in the right place, which is worth knowing
before you move it again. It was `tests/ci_aggregate_gate.rs`, then an
`am-ci-guard` dev-dependency (#156, #157), then a `ci:gate` subcommand built
into `chore` itself. The last two were wrong the same way: they made a release
of a shared tool a prerequisite for a change here, and `chore` is a
general-purpose task runner, not this project's utility bin. The first was
wrong too, for a subtler reason — it parses a YAML file and compares strings,
exercising nothing this crate ships, and as a `cargo test` it **counted towards
the executed-test floor the gate itself enforces**, so the suite could satisfy
its own floor partly by checking its own CI config.

`release.yml` triggers on a `v*.*.*` tag and `fuzz.yml` is dispatch plus a
nightly cron, so neither ever reports on a pull request and neither may gate.

## Build environment

Rust **1.95.0**, pinned in `rust-toolchain.toml` with `rustfmt` and `clippy`,
because a floating `stable` turns a newly added clippy lint into a hard CI
error without `Cargo.lock` moving.

This crate sits at the bottom of the dependency graph: the default build has
no dependency at all (`[dependencies]` holds only clap, clap_complete and
clap_mangen, each optional behind the `cli` feature), there is no `../sibling` path dependency, and nothing has to be checked
out beside it to build or test it. That is why `pre-commit.d/rust-deps-pinned.sh`
looks half-idle here — its sibling-pinning half has nothing to pin, while its
`Cargo.lock` half applies in full. The README says so at length so nobody has
to work it out twice; it is expected, not a misconfiguration.

`chore staticlib` cross-builds `aarch64-apple-darwin` and adds that rustup
target itself. Nothing else in the repository needs a second target.

## Every consumer pins this crate twice, and the second pin is a checkout

Twelve crates depend on `rust-fs-core`, and a consumer declares it as **both** a
version and a path:

```toml
rust-fs-core = { path = "../rust-fs-core", version = "0.2.10" }
```

The `version` is what a published build resolves and what makes a tagged
release reproducible; the `path` is what makes a local build compile the
working tree beside it. Both are load-bearing, and they can disagree. A sibling
checkout whose version is semver-ahead of the consumer's lock makes any cargo
command re-resolve and rewrite that consumer's `Cargo.lock`, and its
`rust-deps-pinned` guard then blocks a commit over a file the commit never
contained. CI closes the gap by cloning this repository **at a tag**
(`git clone --depth 1 --branch v0.2.10 …`), never at `main`.

The practical consequence for an agent: **a change here is not finished when
this repository is green.** Landing a behaviour change means cutting a release
and moving each consumer's pin, which is a separate pull request per consumer.

## The decision that was open: a write past the end, #147 and #129

Read this before touching `FileDevice`, the `BlockDevice` contract, or anything
a write path reaches. **It is settled** — #70, #75, #147 and #129 are all
closed, the replacement shipped, and every consumer has adopted it. It is
written down because the shape recurs, and because the obvious "fix" is still
wrong.

`write_at` **defaults to `Err(Error::ReadOnly)`** on the `BlockDevice` trait —
with `flush` a no-op and `is_writable` returning `false` — so the default
device is strictly read-only and a type opts into writing by overriding all
three. `4e19fc9` (#75) then gave `FileDevice::write_at` a bound of its own: a
write whose end is past `self.size` returns `Error::OutOfBounds { offset, len,
size }` instead of extending the file.

**That change is right.** Before it, a write straddling the end grew the
backing file while `size_bytes()` went on reporting the length taken at
construction, so one device's two halves disagreed about where it ended.
Measured on a 4096-byte file opened with `open_rw`: `write_at(4094, &[1u8; 8])`
returned `Ok`, the file on disk became 4102 bytes, `size_bytes()` stayed 4096,
and `read_at(4096, 6)` handed those six bytes straight back — bytes a caller
bounding its reads by `size_bytes`, which is exactly what `CachingDevice` does
when it clamps every block it fetches, can never reach. That is **#70**.

**And it removed the only way four crates allocate, with no replacement.**
Appending is how a sparse image format grows, and `rust-img-vhd`,
`rust-img-qcow2`, `rust-img-vhdx` and `rust-img-vmdk` each had nothing else.
Every failure was the same shape — a write landing exactly at the device's
current end, which is what allocating a new block, cluster or grain looks like
in all four formats:

```
OutOfBounds { offset: 67108864, len: 1048576, size: 67108864 }
```

**Do not "fix" this by reverting #75 — that reintroduces #70.** It is the
tempting move precisely because it would turn several repositories green again
in one commit.

### What closed it

`BlockDevice::set_len` and `BlockDevice::can_grow` (`e202e7d`, #161), released
in **v0.2.12**. Growth is a named operation that moves the file's length, the
number `size_bytes` reports and the cache's view of it together, and refuses on
a device that cannot grow — a read-only file, a slice, the default
implementation. Both methods are defaulted (`Err(Error::ReadOnly)` and
`false`), so nothing that already implemented the trait had to change.

`tests/device_growth.rs` is the evidence, and it is shaped around the failure
mode rather than the feature: a `set_len` that moves the file and leaves
`size_bytes` — or a cached block — behind **re-creates #70 exactly**, and
passes a naive test, because the write it enables succeeds and only a later
cached read finds the hole. Hence
`a_device_grown_through_the_cache_reads_back_through_the_cache`,
`an_appended_block_written_through_the_cache_reads_back` and
`a_shrink_and_regrow_through_the_cache_does_not_serve_the_bytes_that_went`.

### The consumers moved, which is the half that used to be missing

The old text here said nothing was broken only because ten consumers were
pinned below `4e19fc9`, and that the bill would arrive for whoever next bumped
a pin. It arrived and was paid. Read from each repository's default branch on
2026-09-28 — `Cargo.toml` requirement and `Cargo.lock` agreeing, `ci-ok` green
on the head commit:

| consumer | `rust-fs-core` |
|---|---|
| `rust-fs-ext4`, `rust-fs-xfs`, `rust-fs-btrfs`, `rust-fs-ntfs` | 0.2.13 |
| `rust-fs-erofs`, `rust-fs-squashfs`, `rust-partitions` | 0.2.13 |
| `rust-img-qcow2`, `rust-img-vhd`, `rust-img-vhdx`, `rust-img-vmdk` | 0.2.13, allocating through `set_len` |
| `rust-blk-probe` | `0.2`, locked at 0.2.6 — the one left, and not blocked by this |

The four image writers were the whole of #147, and each adopted the growth
operation before it moved its pin rather than after. `rust-blk-probe` is behind
on every sibling it pins, and is held by antimatter-studios/rust-partitions#131
rather than by anything here (#168).

**The standing rule survives the decision:** a behaviour change here is not
finished when this repository is green. Tag a release, then move each
consumer's pin, one pull request per consumer.

## Never grow a shared tool to solve a problem in this repository

> **Never grow a shared tool to solve a problem in this repository.** `chore`
> is a general-purpose task runner this project merely consumes; the same goes
> for `github-guard` and the agent-skills hooks. If something needed here looks
> like it belongs inside one of them, it does not. Solve it here, or ask first.
> The tell is a release: if a shared tool needs a new version cut whose only
> purpose is to unblock this project, the code is in the wrong repository.

This repository is the one where the rule is easiest to get backwards, because
it also *publishes* something the family consumes — `scripts/output-budget.sh`
and `.github/actions/install-chore`. Those are this crate's own artefacts, and
widening them for a sibling's convenience is the same mistake pointing the
other way. The test is unchanged: whose problem is being solved.

## The shared block above is checked

`scripts/agents-core-check.sh` hashes the content between the BEGIN/END markers
and compares it with the canonical digest, and checks that the digest in the
marker agrees with what follows it. It runs in `chore lint` and in the `fmt`
job of `ci.yml`, so a repository that falls behind the family fails rather than
quietly running last month's rules.

`tests/scripts/agents-core-check.sh` is the test *of* that check — a gate that
cannot fail is indistinguishable from no gate — and `chore test:scripts` runs
it. When the block changes it changes everywhere: edit the canonical copy, then
update the digest in the script and in the BEGIN marker of every repository
that carries it.
