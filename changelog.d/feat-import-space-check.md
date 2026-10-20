### Added

- **Imports of a known size refuse a staged copy that cannot fit.** The additive `FileSink::space` reports free space and the staged file's data-plus-metadata need; before the first write, an import refuses with exit 1, leaving the old file intact and no staged name, when a replacement cannot fit beside the file it replaces. The message states the needed and free bytes. Sinks keep the default, which skips the check.
