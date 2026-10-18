### Added

- **Imports of an unknown size check free space as the staged copy grows.** A stdin import asks `FileSink::space` before each written chunk at the staged copy's new length, and refuses with exit 1, leaving the old file intact and no staged name, once the copy would not fit beside the file it replaces. The message states the needed and free bytes. Running out of space between checks still aborts cleanly through ENOSPC.
