### Fixed

- **Tree exports refuse destination name collisions.** Source names are probed on each destination directory's filesystem before its children are copied. The first listed name is kept; later case variants or other equivalent names are reported as `refused: collides with <name>`, even with `--replace`. Case-sensitive destinations retain distinct names, and refused directories are never traversed.
