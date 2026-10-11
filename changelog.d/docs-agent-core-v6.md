### Added

- **`scripts/stack-base.sh` says which branch a pull request is based on.**
  It prints `main`, or the branch of the newest older open pull request the
  current branch would conflict with or is already built on. Overlap is a
  conflict found by a trial merge (`git merge-tree`), not a shared file, so
  pull requests that change one file in different places are not serialised.
  Every agent asking about the same branches gets the same answer.
- **Changelog fragments: `scripts/changelog-check.sh` and
  `scripts/changelog-gather.sh`.** A pull request adds `changelog.d/<branch>.md`
  instead of editing `CHANGELOG.md`, so open pull requests never conflict
  there. `changelog-check.sh` (lint and CI) refuses a malformed fragment and
  an unreleased entry written into `CHANGELOG.md`. `changelog-gather.sh
  VERSION` is the release step: it writes every fragment into a dated
  section, Keep a Changelog's headings first, and deletes the fragments in
  the same commit. rust-fs-core keeps its own changelog this way from now on.

### Changed

- **The shared agent-core block is v6.** It adds three rules: a pull request
  that would conflict with an older open one is stacked on it, as
  `stack-base.sh` decides; changelog entries are fragments; and each finding
  becomes its own issue before a tracking issue closes. Every repository
  carrying the block moves to v6 with its rust-fs-core pin, and adopts
  `changelog.d/` with `changelog-check.sh` in its lint.
- **`family-check.sh` refuses a copy of the three new scripts**, as it does
  the other family scripts: they run in place from rust-fs-core.
