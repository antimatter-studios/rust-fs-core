#!/usr/bin/env bash
# package-contents.sh -- the published crate carries the family scripts. A
# consumer with no checkout of this repository gets them from the published
# crate, through cargo, so the package must carry them. CI's test jobs and
# `chore check:package` both run this.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# --allow-dirty so an uncommitted tree can be proved. It only lists: on CI's
# clean checkout the list is the same without it.
files="$(cargo package --locked --allow-dirty --list)"
for script in core.sh output-budget.sh test-floor.sh semver-check.sh family-check.sh guest-rust-toolchain.sh stage-siblings.sh guest-rust-run.sh ci-gate.sh package-cli.sh; do
    grep -Fqx "scripts/$script" <<<"$files" || {
        echo "package omits scripts/$script" >&2
        exit 1
    }
done
