### Added
- An opt-in recursive import adapter records, for each verified and published file, the opened local source identity, the driver-supplied destination identity and the verified SHA-256, syncing each entry before traversal continues; unverified tree imports record nothing, and a manifest capture or append error stops the traversal.
