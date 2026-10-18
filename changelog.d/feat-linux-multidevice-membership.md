### Changed
- Linux transfers resolve Btrfs multi-device membership from sysfs, so imports into any member are refused and unrelated block targets stay usable while Btrfs is mounted. ZFS mounts still fail closed (#375).
