#!/usr/bin/env bash
# Check that a repository keeps its unreleased features-page rows as fragments.
#
# A pull request does not edit docs/features.md: it adds one file under
# features.d/, named for its branch, holding the rows it adds, replaces,
# removes or renames, and a release applies them to the page and deletes them
# in the same commit (features-gather.sh). Pull requests open at the same time
# then never conflict over neighbouring rows of one table in the merge queue.
#
# This is what keeps that true, run by `chore lint`:
#
#   - features.d/ exists, and holds only fragments (*.md) and its README.md;
#   - every fragment applies to the page as it stands: each `## Section` it
#     names is a section of docs/features.md with one table, each row has
#     that table's number of columns, each `remove:` and `rename:` names a
#     key the section holds exactly once, and no two fragments write the
#     same key of the same section;
#   - docs/features.md holds no Unreleased row of its own: no Since cell says
#     `Unreleased`, because an unreleased row is a fragment until a release
#     gathers it.
#
# A repository with no docs/features.md has nothing to check, and passes.
#
# The rules and the parser are features-gather.sh's own, run with --check, so
# a fragment this passes is one the release can apply: there is one reading
# of a fragment, not two that can drift.
#
# Usage: features-check.sh            (the calling repository)
#   FS_CORE_CALLER names the repository when run in place from rust-fs-core.
set -euo pipefail

[ $# -eq 0 ] || { echo "usage: features-check.sh" >&2; exit 2; }
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec bash "$HERE/features-gather.sh" --check
