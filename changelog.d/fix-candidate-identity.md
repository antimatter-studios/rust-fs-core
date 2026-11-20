### Fixed

- **Candidates packed into one record are told apart.** The provided method
  `SalvageAdapter::candidate_id` names a candidate in trees and parent
  claims. It defaults to `location.offset`, so existing drivers are
  unchanged. A driver whose records share source bytes overrides it: SquashFS
  inodes in one compressed metadata block use their inode reference. The
  conformance checks key candidates on it. A rescan finds a packed candidate
  among its record's candidates, and a scan stopped among them may resume at
  the record and deliver them again, but loses none (#467).
