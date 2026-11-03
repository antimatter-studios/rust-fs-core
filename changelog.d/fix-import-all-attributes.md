### Fixed
- Tree imports with `--preserve=xattrs` offer every listed host attribute even when a sink refuses an earlier one, and name every attribute write error in the item's existing reason. Accepted attributes and published contents remain intact; the first error's classification, report schema, and partial-transfer exit status are unchanged.
- macOS `com.apple.provenance` remains an opaque attribute offered to the sink, not silently filtered as host bookkeeping. The existing `com.apple.ResourceFork` named-stream handling is unchanged.
