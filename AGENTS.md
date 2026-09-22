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

<!-- BEGIN SHARED BLOCK: agent-core v1 sha256:60fad6dd98e9da3e9256d38728b02ac189dca0d04fc98c13e2c67de3f3103319 -->
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
that log, and a failure prints its tail. CI keeps the logs as an artifact, so
the detail is always retrievable.

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

## Project rules

- **No GPL/LGPL/AGPL dependencies.** Permissive only (MIT/BSD/Apache).
  Shelling out to a copyleft CLI as a *test oracle* is fine — linking or
  copying it is not.
- **Each of these is a standalone project.** Never mention a consuming
  application in the README, the source, or CLI help.
<!-- END SHARED BLOCK: agent-core v1 -->

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
- An **executed-test floor of 330** on every tier (`scripts/test-floor.sh`). A
  suite that silently stops running a material part of itself otherwise reports
  exactly the same green as one that passes.

## How the budget is wired here — this repository owns the wrapper

`scripts/output-budget.sh` is the canonical, family-neutral copy. Ten siblings
resolve it from their pinned `../rust-fs-core` checkout and validate it by
SHA-256 and API version; none of them may carry its own copy, and none of them
may edit this one to suit itself. `docs/output-budget.md` is the contract.
`#153` is the open work on the copies that diverged before that was true.

Locally it is reached through two thin adapters that do belong here:

- `scripts/tier.sh LABEL LOG MAX-LINES MAX-BYTES -- CMD...` runs a tier
  quietly into `tmp/logs/<LOG>.log`, printing one verdict line on success and
  the tail on failure.
- `scripts/test-floor.sh <LOG> <n>` reads that same log back and fails when
  fewer than `n` tests executed.

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
other job in `ci.yml` — `test` (the ubuntu / macOS / Windows matrix), `fmt`
and `coverage` — and fails when any of them failed, was cancelled or was
**skipped**. It runs no tests of its own, deliberately: it is a claim about the
other jobs, so it must not be able to pass work of its own off as theirs.

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

**`tests/ci_aggregate_gate.rs` holds both halves mechanically** — every job in
`ci.yml` must appear in `ci-ok`'s `needs:`, and `.github-guard` must require
`ci-ok` and nothing else.

It briefly lived elsewhere and came back, which is worth knowing before you
move it again. It was an `am-ci-guard` dev-dependency (#156, #157), then a
`chore ci:gate` task, and both were wrong for the same reason: a checker that
reads two text files and compares strings is not this crate's code, and putting
it in a shared tool made a release of that tool a prerequisite for a change
here. #158 and #159 reverted both. It is a test in this repository, and that is
where it stays.

`release.yml` triggers on a `v*.*.*` tag and `fuzz.yml` is dispatch plus a
nightly cron, so neither ever reports on a pull request and neither may gate.

## Build environment

Rust **1.95.0**, pinned in `rust-toolchain.toml` with `rustfmt` and `clippy`,
because a floating `stable` turns a newly added clippy lint into a hard CI
error without `Cargo.lock` moving.

This crate sits at the bottom of the dependency graph: `[dependencies]` is
empty, there is no `../sibling` path dependency, and nothing has to be checked
out beside it to build or test it. That is why `pre-commit.d/rust-deps-pinned.sh`
looks half-idle here — its sibling-pinning half has nothing to pin, while its
`Cargo.lock` half applies in full. The README says so at length so nobody has
to work it out twice; it is expected, not a misconfiguration.

`chore staticlib` cross-builds `aarch64-apple-darwin` and adds that rustup
target itself. Nothing else in the repository needs a second target.

## Every consumer pins this crate twice, and the second pin is a checkout

Twelve crates depend on `am-fs-core`, and a consumer declares it as **both** a
version and a path:

```toml
am-fs-core = { path = "../rust-fs-core", version = "0.2.10" }
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

## The live open decision: a write past the end, #147 and #129

Read both issues before touching `FileDevice`, the `BlockDevice` contract, or
anything a write path reaches.

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
Measured, each on its own unmodified `main`, against this crate's `main` versus
its pinned tag:

| crate | against `main` | against the pinned tag |
|---|---|---|
| `rust-img-vhd` | 62 passed, **7 failed** | 69 passed, 0 failed |
| `rust-img-qcow2` | 48 passed, **3 failed** | 48 passed, 0 failed |
| `rust-img-vhdx` | 37 passed, **1 failed** | 38 passed, 0 failed |
| `rust-img-vmdk` | 24 passed, **1 failed** | 25 passed, 0 failed |

Every failure is the same shape — a write landing exactly at the device's
current end, which is what allocating a new block, cluster or grain looks like
in all four formats:

```
OutOfBounds { offset: 67108864, len: 1048576, size: 67108864 }
```

Nothing is broken today only because **ten consumers are pinned** to a tag that
predates `4e19fc9`. The bill arrives for whoever next bumps a pin, and they
meet it as a pile of failing write tests with no obvious connection to the
bump.

**Do not "fix" this by reverting #75 — that reintroduces #70.** It is the
tempting move precisely because it turns ten repositories green again in one
commit. The remedy under discussion is an explicit growth operation on the
writable device contract: `BlockDevice::set_len` / `grow_to`, or a separate
growable trait, which moves the size `size_bytes` reports, the cache's view of
it and the file length together, and refuses on devices that cannot grow — a
raw block device or a slice. Pre-sizing in each consumer is the third option
and is the weakest: it is workable for a fixed image and awkward for a sparse
one that grows as it is written, which is the whole point of the format.

This is a decision about **this crate's contract**, which is why it is open
here rather than settled four different ways downstream. Land it with a test
that a device grown that way reads back through `CachingDevice`, and **tag a
release before any consumer bumps its pin**.

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
