### Added

- **Transfers stop cleanly on SIGINT and SIGTERM on Unix.** Chunk and publication checks remove the current staging file, retain completed destinations, and report completed, interrupted and pending items with status 130 or 143. Repeated signals preserve the first cancellation and cannot bypass cleanup; stdout content keeps its report on stderr.
