### Added

- **`fs_core::cli::transfer` copies a file out of an image, bounded and
  digested.** A driver implements `FileSource` (a length and a positioned
  read) and wires `export_args()` and `export()` into its `export` verb. The
  copy moves 1 MiB at a time through one buffer, reports its byte count and
  SHA-256, writes a local file under a hidden staging name and publishes it
  only when complete: by hard link, refusing an existing name, or by rename
  with `--replace`. A failed read or write leaves any existing destination as
  it was, and a failed writer reports how many bytes of the total it took.
  The `cli` feature now also turns on `sha2` (MIT/Apache-2.0).
