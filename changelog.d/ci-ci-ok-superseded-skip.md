### Changed

- **ci-gate accepts one condition besides `if: always()` for `ci-ok`.**
  `!(cancelled() && github.event_name == 'pull_request' &&
  !startsWith(github.event.pull_request.title, 'red:'))`, written exactly, is
  false only in a cancelled ordinary pull request run, so a run that a newer
  head superseded completes without waiting for a runner and no longer holds
  the pull request's concurrency group. Its `ci-ok` concludes `cancelled`,
  never `skipped`; a cancelled `red:`, push, merge-queue or dispatch run still
  runs `ci-ok`, and it fails. Every other condition is still refused.
