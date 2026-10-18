### Added
- Recursive imports with `--manifest FILE --resume` apply the single-file resume decision to each file under the one image lock: unchanged verified files are reported `skipped: verified earlier` with their original completion time and do not fail the run, and every other file is staged again from byte zero.
