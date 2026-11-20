### Fixed

- **A compressed unit stored larger than it decodes to passes the walk
  checks.** `conformance::check_primitive` and the salvage set compare a
  walk's file bytes with its claim, and its stored bytes only with the
  source. A short file whose compressed unit carries overhead, such as a
  49-byte Btrfs LZO file stored in 54 bytes, no longer fails (#462).
- **Encoded runs back to back on the source are not reported as unmerged.**
  The salvage set's walk-shape check skips encoded runs, as the
  `ExtentWalk` contract says they are never merged, so a Btrfs file in
  several compressed units placed back to back conforms (#463).
