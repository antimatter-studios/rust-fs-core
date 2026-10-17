### Added

- **Single-file imports stream through driver-owned staging.** The additive `FileSink` and shared import arguments accept local files or stdin with bounded memory, refuse unsupported sinks and atomic replacement before consuming input, persist before publication, abort failed or cancelled copies, and report bytes, SHA-256 and creation or replacement.
