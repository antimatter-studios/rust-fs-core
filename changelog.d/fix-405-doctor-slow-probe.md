### Fixed

- **`doctor` waits longer for a slow program (#405).** A program of ours that
  took more than five seconds to answer `--version` on a loaded host was
  reported as `foreign` with no version. It now has twenty seconds.
