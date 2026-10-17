### Added

- **Exports can verify the synced staging file before publication.** The
  additive `ExportOptions`, `export_args_with_verify`, `export_with_options`
  and `export_file_with_options` APIs enable `--verify`: bounded read-back
  must match the copied byte count and SHA-256 before the destination is
  published. A mismatch reports both digests, removes staging and preserves
  an existing destination. Verification of stdout is a usage error (exit 2),
  and file reports claim `verified: true` only after successful read-back.
  Existing export APIs and public struct fields remain compatible.
