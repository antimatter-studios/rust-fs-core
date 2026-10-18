### Added
- Optional named-stream source and sink capabilities under `--preserve=xattrs`, with explicit named refusals when the destination cannot represent streams.

### Changed
- Tree exports refuse named streams without creating side files; host stream import remains separate work under #354.
