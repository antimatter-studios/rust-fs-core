# fs-core

Pure-Rust block-device framework. The shared substrate every filesystem
driver and disk-image reader plugs into.

## What it gives you

- `BlockRead` — read-only random-access block device (`read_at` + `size_bytes`)
- `BlockDevice: BlockRead` — adds optional `write_at` / `flush` / `is_writable`
- `FileDevice` — backed by a regular file, optional read-only
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
```

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

## License

MIT.
