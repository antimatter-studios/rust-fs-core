### Added

- **Transfers report progress only on stderr.** Existing export entry points
  automatically rewrite one terminal line and remain silent when stderr is
  piped. Additive progress arguments and entry points enable JSON completion
  lines per regular file, with intermediate updates limited to one per second.
  File and overall logical byte counts include known totals before copying;
  tree totals come from metadata listings, and unavailable totals stay unknown.
  Content on stdout and final reports remain unchanged. Link and timestamp
  preservation retain their existing opt-in behavior.
