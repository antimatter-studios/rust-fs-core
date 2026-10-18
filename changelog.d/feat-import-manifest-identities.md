### Added
- An opt-in single-file import adapter records the opened local source identity, the driver-supplied destination identity captured under the image lock and the verified SHA-256 only after read-back verification and publication, syncs the entry before returning, and reports torn-tail recovery.
