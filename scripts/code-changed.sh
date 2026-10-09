#!/usr/bin/env bash
# code-changed.sh -- does a change need the full pipeline? Reads the
# change's paths on stdin, one per line, and prints `true` if any of them is
# not documentation, `false` if every one is. Run in place from the
# rust-fs-core sibling by a repository's `changes` job; this file is the only
# copy, so every repository in the family skips for the same reasons.
#
# DOCUMENTATION is a markdown file anywhere (`*.md`), the licence
# (`LICENSE*`) and agent settings (`.claude/**`). Nothing else is listed:
# a new kind of file, a path that cannot be read, and an empty list all mean
# the full pipeline, so the one way this can be wrong costs a run, never a
# test that should have run. Scripts, task files, manifests and workflows
# are code: a line in a fixture script changes every oracle result.
#
#   git diff --name-only origin/main...HEAD | bash scripts/code-changed.sh
set -uo pipefail

any=false
while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    any=true
    case "$path" in
        *.md | LICENSE* | */LICENSE* | .claude/*) ;;
        *)
            echo true
            exit 0
            ;;
    esac
done
if [ "$any" = true ]; then
    echo false
else
    echo true
fi
