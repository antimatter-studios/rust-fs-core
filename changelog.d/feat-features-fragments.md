### Added

- **Features-page rows are fragments, like changelog entries.** A pull
  request adds a `features.d/` fragment of `docs/features.md` rows, keyed by
  their first cell, with `remove:` and `rename:` lines, instead of editing the
  page, so pull requests that change different rows no longer conflict in the
  merge queue. `scripts/features-check.sh` (for `chore lint`) refuses a
  fragment that does not apply and an `Unreleased` row on the page;
  `scripts/features-gather.sh VERSION` (for the release commit) applies every
  fragment, turns `Unreleased` into the version and deletes the fragments, or
  changes nothing. Both are family scripts, and `docs/features-fragments.md`
  holds the adoption steps.
