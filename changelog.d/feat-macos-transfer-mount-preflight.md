### Added
- macOS transfers inspect host mount state for image files and disk devices, using attached disk images, their slices and synthesized APFS containers. Imports refuse mounted targets before taking the lock or consuming input; exports warn on read-write mounts. Failed hdiutil, diskutil or mount probes fail closed.
