### Added

- **Tree exports can preserve access and modification times.** Additive time
  options and `--preserve=times` restore directories after their descendants
  and verify results through host metadata. Rounded, clamped or rejected times
  fail the affected item while preserving its published data; pre-epoch and
  unavailable source times are reported. Times can be combined with link
  preservation; symbolic link times are left unchanged and reported without
  following targets. Existing entry points retain host timestamps.
