### Added

- **Manifest evidence can authorize an earlier-verification skip.** The shared CLI ledger API requires current endpoint identities and a bounded fresh SHA-256 read of the source, consults only the latest destination record, and returns the original completion time. Missing, changed, partial or superseded evidence requires copying from byte zero. Access-time changes alone do not invalidate evidence. Export and import `--resume` command integration follows separately.
