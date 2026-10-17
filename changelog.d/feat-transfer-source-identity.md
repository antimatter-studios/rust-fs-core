### Added

- **Local imports reject sources that change during copying.** The additive `LocalSource` and `import_local_file` API record an open file's identity, length and modification time, plus Unix change time, then recheck the handle and optional source name after copying and persistence. Changes abort staging and preserve the destination; successful reports retain the read-byte SHA-256 and name the restored-mtime detection limitation on hosts without change-time support.
