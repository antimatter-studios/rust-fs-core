### Added

- **Imports can verify the persisted staged file before publication.** The
  additive `ImportOptions`, `import_args_with_verify` and
  `import_with_options` APIs enable `--verify`: after persistence the staged
  file is read back through the sink in bounded chunks and must match the
  sent byte count and SHA-256 before it is published. A mismatch aborts the
  staged file, preserves an existing destination and reports both digests.
  Import reports now carry `verified`, true only after a successful
  read-back. Existing import APIs and request fields remain compatible.
