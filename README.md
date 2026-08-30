# fs-core

Pure-Rust block-device framework. The shared substrate every filesystem
driver and disk-image reader plugs into.

## What it gives you

- `BlockRead` — read-only random-access block device (`read_at` + `size_bytes`)
- `BlockDevice: BlockRead` — adds optional `write_at` / `flush` / `is_writable`
- `FileDevice` — backed by a regular file, optional read-only
- `CallbackDevice` — backed by host-process-owned callbacks (FFI from
  Swift / Go / C++)
- `CachingDevice` — LRU read-cache decorator wrapping any `BlockDevice`
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
- `IoStats` hook — counters for reads/writes, bytes-read/bytes-written,
  cache hit rate. `CachingDevice` already exposes some of these
  internally; the plan is to generalise across every adapter.

## License

MIT.
