### Added

- **Verified transfer evidence has durable storage.** The optional CLI manifest API writes typed local and image identities, byte counts, SHA-256 and completion timestamps as synced JSON lines. It excludes unverified attempts, refuses concurrent writers, and reports and removes a torn final line before appending again. Export and import command integration follows separately.
