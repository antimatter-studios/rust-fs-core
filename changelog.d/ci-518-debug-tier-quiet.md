### Fixed

- `scripts/tier-progress.sh` counts the failed tests in a log from `cargo test -- --quiet`, and no longer counts a test binary's `test result: FAILED.` line as a failed test.
