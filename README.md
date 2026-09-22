# fs-core

Pure-Rust block-device framework. The shared substrate every filesystem
driver and disk-image reader plugs into.

## What it gives you

- `BlockRead` — read-only random-access block device (`read_at` + `size_bytes`)
- `BlockDevice: BlockRead` — adds optional `write_at` / `flush` /
  `is_writable`, and `set_len` / `can_grow` for the devices that can change
  their own length
- `FileDevice` — backed by a regular file, optional read-only; the one device
  here that can grow, when it is open read-write on a regular file
- `CallbackDevice` — backed by host-process-owned callbacks (FFI from
  Swift / Go / C++)
- `CachingDevice` — LRU read-cache decorator over a `BlockRead`, and over a
  `BlockDevice` when the caller has one to give (`new` vs `read_only`)
- `CountingDevice` — counts the reads and the bytes a driver asks its device
  for, so drivers can be compared against each other and against themselves
  later by one instrument rather than several look-alike ones
- `ReadOnlyDevice` — rejects writes whatever the inner type allows
- `SliceReader` / `OwnedSlice` / `OwnedRwSlice` — a sub-range of a device
  presented as a device, for partition walkers and container readers
- `BlockReadStreamer` — `std::io::Read + Seek` over any `BlockRead`
- A unified `Error` type with a `Custom(String)` escape hatch so each
  driver can lift its own internal errors to the trait boundary

## Intended consumers

Filesystem drivers — `fs-ext4`, `fs-ntfs`, future `am-fs-exfat`,
`am-fs-hfsplus`, `am-fs-apfs`, `am-fs-fat32`, `am-fs-fat16`,
`am-fs-squashfs`, `am-fs-iso9660`. Each only writes format-specific
code; the block plumbing comes from here.

Disk-image readers — `am-img-qcow2`, `am-img-vhd`, `am-img-vhdx`,
`am-img-vmdk`; future `am-img-vdi`, `am-img-raw`. Same pattern:
format-specific container logic, shared block I/O.

Block-layer utilities — `am-partitions` (GPT/MBR probe), future
`am-block-luks`, `am-block-lvm`, `am-block-mdraid`. Same trait,
opposite direction: consumes a `BlockRead` to expose slices of it.

## Layout

```
src/
  lib.rs              public re-exports
  error.rs            Error / Result
  block.rs            BlockRead + BlockDevice traits
  file_device.rs      FileDevice (backed by std::fs::File)
  callback_device.rs  CallbackDevice (FFI-friendly)
  caching_device.rs   CachingDevice (LRU decorator)
  counting_device.rs  CountingDevice (counts reads + bytes asked for)
  slice.rs            SliceReader / OwnedSlice / OwnedRwSlice
  readonly.rs         ReadOnlyDevice (rejects writes whatever the inner type allows)
  stream.rs           BlockReadStreamer (std::io::Read + Seek over a BlockRead)
  ffi.rs              the C ABI — FsCoreDevice, FsCoreCallbackCfg
tests/
  cache.rs            CachingDevice + interop tests
.github/actions/
  install-chore/      the composite action that installs `chore` in CI
```

## The CI gate, and the chore installer

The one required check is `ci-ok`, and it stands for every job — see
`.github-guard`, which argues why at length. What holds that true is
**`chore ci:gate`**, run by the `gate` job: the gate workflow must run on
`pull_request`, `ci-ok` must `needs:` every gating job and nothing that does
not exist, it must carry `if: always()` rather than a narrowing of it, and
`.github-guard` must require `ci-ok` alone.

Those rules were `tests/ci_aggregate_gate.rs` here, and in ten sibling
repositories as ten variants of the same 161–173 lines; then briefly a
`crates/am-ci-guard` dev-dependency (#156, #157). Both were the wrong
container. The rules test nothing this crate ships — they parse a YAML file
and compare strings — and the `test` job enforces an executed-test floor, so
a meta-test inflates the very count used to satisfy the gate. They live in
`antimatter-studios/chore` now, where a consumer cannot locally edit them and
where no cargo dependency graph has to hear about them.

`.github/actions/install-chore` is the composite action that installs the
binary (antimatter-studios/chore#52), for the other thing every repository
was writing out by hand:

```yaml
- uses: antimatter-studios/rust-fs-core/.github/actions/install-chore@<ref>
  with:
    version: "0.11.0"
```

Three repositories each carried a copy of `scripts/ci-install-chore.sh`, one
inlined the asset mapping four times, and a fifth hand-written copy asked for
a tarball name that did not exist and 404'd on every scheduled run. The
convention belongs to whoever publishes the releases, so it is spelled once,
checksum-verified. Pin the `@ref`.

## Roadmap

Planned additions (not yet implemented):

- `Logger` hook — pluggable `set_logger(callback)` so consuming crates
  can route diagnostics to a host-provided sink without each crate
  hard-coding a logging dependency.
- `IoStats` hook — the **write** side of the counters, and a hit rate
  generalised beyond `CachingDevice::stats`. The read side has shipped:
  `CountingDevice` counts reads and bytes-read for any `BlockRead`, and
  `CachingDevice::stats` reports its own hits and misses. What is left is
  writes/bytes-written, and one accessor that reads the same way across
  every adapter rather than per type.

## Git hooks

The guards are [github-guard](https://github.com/antimatter-studios/agent-skills)'s,
installed once per clone into `.git/hooks`:

```sh
~/.claude/skills/github-guard/install.sh .
```

Nothing is committed for them: a hook inside the working tree is a hook a
branch checkout can replace, which is what moving them out of it prevented.
The one tracked file is `.github-guard`, which declares the checks `main`
requires.

### Why the dependency-pinning guard looks half-idle here

`pre-commit.d/rust-deps-pinned.sh` is the same file every sibling
project runs, and part of it has nothing to do in this one. That is expected
rather than a misconfiguration, and it is recorded here so nobody has to work
it out twice.

The guard does two jobs. It refuses a workflow that clones a **sibling
project** at a floating ref instead of a tag — and this crate sits at the
bottom of the dependency graph with an empty `[dependencies]`, so there is no
sibling to pin and that half never fires. It also refuses a missing or drifted
`Cargo.lock` and runs `cargo metadata --locked` as a stale-lock check, and
that half applies in full, because this crate does track its lockfile.

The guard is the same in every repository deliberately. The value of a
shared guard is that one audit covers every repository, so a local edit to
trim the idle half would cost more than the idle half does. It is updated by
re-running github-guard's installer, never by editing the copy in `.git/hooks`.

## License

MIT.
