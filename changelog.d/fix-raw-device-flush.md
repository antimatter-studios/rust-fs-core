### Fixed
- `FileDevice::flush` on macOS falls back to `fsync` when `F_FULLFSYNC` returns `ENOTTY`, allowing raw character devices to flush while preserving all other synchronization errors.
