### Changed
- Linux transfers resolve ZFS pool membership from `zpool status -P`, so imports into any vdev of an imported pool are refused and unrelated block targets stay usable while ZFS datasets are mounted. A missing or failing `zpool` fails closed (#376).
