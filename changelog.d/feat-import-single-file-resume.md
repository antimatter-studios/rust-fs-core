### Added
- Single-file imports with `--manifest FILE --resume` skip a file only when fresh source and driver destination identities, a complete source digest and a post-digest revalidation all match the latest record, reporting `skipped: verified earlier` with the original completion time; any other evidence stages the file again from byte zero.
