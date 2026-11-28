### Added

- `scripts/tier.sh progress [LOG-NAME]` prints how far each tier has got, as `scripts/tier-progress.sh` does.
- A tier that passes records its duration and each test binary's duration in `tmp/logs/<log>.history`, and the next run's ETA is worked out from it.

### Changed

- `scripts/tier-progress.sh` reports a cargo tier's phase: crates compiled against `Cargo.lock` while it builds, and tests done against the total known so far while it tests. An ETA with no earlier run to use says it comes from the rate alone.
