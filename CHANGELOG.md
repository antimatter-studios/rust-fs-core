# Changelog

Notable changes to `am-fs-core`, newest first. This is a `0.x` crate, so the
**minor** is the compatibility boundary: a minor bump may break API, a patch
never does.

Every other driver in this family depends on this crate, so a change here
reaches all of them.

## [Unreleased]

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
