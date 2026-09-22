# Output-budget contract

`scripts/output-budget.sh` is the one canonical neutral wrapper for the
filesystem-driver family. It has no dependency on a VM harness, `chore`, or
Rust tooling. It only requires Bash and ordinary Unix utilities.

Every driver-family repository already resolves a pinned `../rust-fs-core`
sibling. Its repository-local tier adapter must invoke
`../rust-fs-core/scripts/output-budget.sh` directly. Consumers must not copy or
vendor the script: the pinned core checkout is both the dependency and the
version boundary, so fixes land once and every consumer receives them by
updating its core pin.

## Resolver contract for standalone clones

A standalone consumer checkout may not have initialized any siblings. The
consumer therefore checks in a small resolver, not a copy of this script. The
resolver has this order:

1. If `../rust-fs-core/scripts/output-budget.sh` exists, return it directly.
   This deliberately permits a developer to test coordinated, uncommitted
   core and driver changes.
2. Otherwise ask Cargo for the resolved `am-fs-core` package. Package-based
   resolution requires `am-fs-core` **0.2.11 or later**, because published
   crate archives are immutable. The package must contain this script and the
   resolver still verifies its API version and SHA-256.
3. Otherwise look in a user or repository tooling cache under a key containing
   the consumer's pinned full core commit, script API version, and expected
   SHA-256. Reuse the entry only after recomputing the digest and checking
   `--version`.
4. Otherwise download `scripts/output-budget.sh` from an immutable raw URL at
   that full core commit into a temporary file, verify its SHA-256 and
   `--version`, make it executable, then atomically install it in the cache.
5. Print only the resolved absolute path. Diagnostics go to stderr so command
   substitution remains safe.

The resolver's pin and digest are consumer build metadata and must move in the
same change as the consumer's core pin. A mutable branch or tag URL is not a
version boundary. A digest fetched from the same mutable location as the
script is not an integrity check.

Offline + no sibling + no valid cached artifact cannot satisfy all three goals
(one canonical source, no checked-in copy, standalone operation). The resolver
must fail clearly in that case, naming the expected sibling path, pinned core
commit, cache path, and the command that can populate the cache. It must not
silently run an unverified script or fall back to an embedded copy.

The stable behavior is:

- a passing command is quiet and prints one verdict naming the full log;
- a failing command prints the log tail and returns the command's status;
- a passing command over its measured line or byte budget returns 65;
- `--verbose` or `OUTPUT_BUDGET_VERBOSE=1` streams output without lifting the
  budget.

Each consumer still owns its adapter, log location, measured budgets,
executed-test floors, and CI artifacts. Pointing at this script alone does not
make a consumer's test suite quiet or prove that its tests ran.

`tests/output_budget.rs` behavior-tests the canonical script here. Consumer
tests should verify that their tier adapter resolves the pinned sibling and
that every tier carries non-zero budgets and a floor; they should not duplicate
the script's behavior suite.

The Windows and Linux VM harnesses are infrastructure, not owners of another
permanent copy. A driver uses the pinned core script directly for local runs.
When it stages its test suite into a harness runtime, the staging step
materializes a fresh transient copy from the resolver's verified result,
records the core commit/API version/SHA-256 in the run manifest, and discards
the file with the run. The harness repository does not vendor the script and
must not reuse a prior run's staged file.

A future mechanism outside both the driver and harness dependency graphs could
also provide the runtime wrapper. Until then, the direction stays explicit:
core owns the script, drivers own their tier policy, and harnesses only receive
the staged runtime material.

## Why the wrapper did not move into the shared CI tooling (#156)

The same kind of duplication was ended one layer up: `tests/ci_aggregate_gate.rs`
existed in eleven repositories as ten distinct files, and those rules are now
`chore ci:gate` — one source, in `antimatter-studios/chore`, that a consumer
cannot locally edit. (They were briefly `crates/am-ci-guard` in this
repository, #157, which is reverted; the container changed, the conclusion
below did not.) The obvious next step is to move this script in there too, so
one dependency delivers everything. It was considered and rejected, and this
section is here so it is not reopened without a new argument.

**The script has to run where neither a crate nor the runner can reach.** `rust-fs-ext4`,
`rust-fs-xfs` and `rust-fs-btrfs` run `chore test:vm`: the suite compiles
and runs *inside the guest*, which has no cargo registry, no `CARGO_HOME`
and no Rust toolchain of its own. #153 names this as the actual blocker —
"the blocker is the guest, not the merge". A Rust entry point — or a Go one,
now — would have to be built for the guest and shipped there, which is
strictly more machinery than transferring one 94-line file that `bash`
already runs. A shell script genuinely cannot be delivered by
`[dev-dependencies]` alone, and that argument does not disappear by
rewriting it in another language: it moves to a harder place.

**A crate already delivers it, and a second route would be the problem
again.** This script is inside the published `am-fs-core` archive — `ci.yml`
has a step that fails the build if `cargo package --list` stops naming it —
and `rust-fs-ntfs/scripts/resolve-output-budget.sh` already resolves it
through `cargo metadata`, verifying the API version and the SHA-256. Adding
a second delivery path would give one file two ways to be reached, which is
the shape of #153 ("sourced four different ways"), not the fix for it.

**Moving the bytes is a cross-repo lockstep, not a refactor.** ntfs's
resolver hard-fails on a digest of this exact file. Relocating it — even
without changing a character — breaks the sibling branch of that resolver
the moment a developer has both checkouts. That belongs with the consumer
migration #153 sets out, in one deliberate sweep, not in the change that
merely creates the crate.

**So: this file stays the one canonical copy, at this path, byte for byte.**
`chore ci:gate` owns the CI *gate*; `scripts/output-budget.sh` owns test
*output*. The remaining divergent copies are all in consumer repositories —
`fs-linux-test-harness`, `rust-img-vhd`, `rust-img-vhdx`, `rust-img-vmdk`
and `rust-img-qcow2` — and deleting them is #153's migration, in the order
it gives, ending with the harness's copy once nothing reads it.
