### Added

- **`scripts/tier-progress.sh` says how far a running tier has got.** One line per tier in the caller's `tmp/logs`: lines against the budget, failed tests, elapsed time, an ETA, and whether its process is running. `tier.sh` leaves a `tmp/logs/<log>.meta` beside the log for it, and prints and logs what it did before. The percentage reads low, because a budget has margin.
