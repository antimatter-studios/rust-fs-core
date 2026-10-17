### Added

- **Directory trees can be exported through `TreeSource`.** The additive recursive export API copies each file through the existing staged transfer, refuses unsafe child names and destination symlinks, bounds depth to 64 by default, merges existing directories and reports every visited item with a nonzero status for any incomplete item. Existing files require `--replace`; verification and backing-image locks remain available.
