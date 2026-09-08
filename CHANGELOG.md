# Changelog

Notable changes to `am-fs-core`, newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.

Every other driver in this family depends on this crate, so a change here
reaches all of them.

## [Unreleased]

### Fixed

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

### Changed

- `SliceGeometry::rebase`'s doc comment no longer claims a check against
  the parent that it never performed. It said it returned `None` "when
  the rebased offset would not fit on the parent at all"; there was no
  parent in scope, and the only thing tested was that the address fit in
  a `u64`. The C header made the matching promise, that the slice's
  addressable range "is `length` bytes", and now states the clamp.

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

[Unreleased]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.4...HEAD
[0.2.4]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/antimatter-studios/rust-fs-core/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/antimatter-studios/rust-fs-core/releases/tag/v0.1.0
