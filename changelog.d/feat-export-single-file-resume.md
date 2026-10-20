### Added
- Single-file exports with `--manifest FILE --resume` skip a file only when fresh source and destination identities, a complete source digest and a post-digest revalidation all match the latest record, reporting `skipped: verified earlier` with the original completion time; any other evidence restarts the copy at byte zero.
