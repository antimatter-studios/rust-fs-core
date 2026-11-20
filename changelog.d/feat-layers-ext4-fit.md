### Changed

- **An extent walk keeps file offsets.** `ExtentWalk::extents` holds
  file-ordered `WalkExtent`s (file offset, source extent, `uninitialized`),
  and a gap between them is a hole, so sparse files and unwritten extents
  survive a walk. `ExtentWalk::plan()` rebuilds the data in place, with zeros
  for holes and uninitialized runs, and `ExtentWalk::contiguous()` builds a
  chain-shaped walk.
- **Parent claims carry the claiming entry's name and state.**
  `TreeNode::parents` holds `ParentClaim`s, so hard links and deleted entries
  are told apart on formats that keep names in directory entries.
- **A damaged redundant copy makes a volume `Degraded`, not `Clean`.** A
  warning finding sets `HealthState::Degraded`, and a clean volume carries no
  findings.
- `read_extent` at an extent's end returns `Ok(0)`.
