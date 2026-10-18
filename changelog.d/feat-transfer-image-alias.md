### Added

- Transfers can refuse local paths that identify their backing image before
  reading bytes or creating staging files. Drivers opt in through the
  additive, defaulted `FileSource::image_path`; `refuse_image_alias` provides
  the same preflight for import sources and every local path in a tree.
  Hard links, symlink chains and Unix device numbers are checked regardless
  of partition offset or `--replace`. Refusals name both paths with exit 1.

### Fixed

- **Per-target floors read a CRLF floors file.** `test-floor.sh --targets`
  strips the carriage return a Windows checkout leaves on each line, which it
  had refused as a non-numeric floor.
