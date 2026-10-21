### Fixed

- **Writable Windows mounts are reported before image export.** Exports now check the host mount state before opening a mounted image, warning for writable mounts and proceeding without a cooperative image lock.
