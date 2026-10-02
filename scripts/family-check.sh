#!/usr/bin/env bash
# family-check.sh -- the calling repository runs the family's scripts from
# rust-fs-core and keeps no copy of its own. Run it as
# `scripts/core.sh family-check`; this file is the only copy.
#
# The family scripts were once copied into every repository and the copies
# drifted, a fix reaching one repository and none of the others. A rule that
# a copy must not come back is only a rule if something fails when one does,
# so every repository runs this in CI, and it checks three things:
#
#   1. NO COPY. scripts/output-budget.sh, scripts/test-floor.sh,
#      scripts/semver-check.sh and scripts/guest-rust-toolchain.sh are not
#      committed in the caller.
#   2. ONE BOOTSTRAP. The caller's scripts/core.sh is byte-identical to this
#      repository's, so the way core is found is the same everywhere.
#   3. NOTHING CALLS A COPY. No workflow, chores.yml or script in the caller
#      runs scripts/test-floor.sh, scripts/semver-check.sh or
#      scripts/guest-rust-toolchain.sh directly; the calls go through
#      scripts/core.sh.
#
# Run here, in rust-fs-core itself, there is nothing to compare against but
# itself, and the checks are skipped by being trivially true -- this
# repository's own tests prove each refusal against caller trees.
set -uo pipefail

FAMILY_CHECK_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-family-check %s\n' "$FAMILY_CHECK_API_VERSION"
    exit 0
fi
[ $# -eq 0 ] || { echo "usage: family-check.sh" >&2; exit 2; }

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CALLER="${FS_CORE_CALLER:-$CORE}"
CALLER="$(cd "$CALLER" && pwd)"

fails=0
fail() { printf 'FAIL  family-check: %s\n' "$1"; fails=$((fails + 1)); }

if [ "$CALLER" != "$CORE" ]; then
    # 1. No copy.
    for script in output-budget.sh test-floor.sh semver-check.sh guest-rust-toolchain.sh; do
        if [ -e "$CALLER/scripts/$script" ]; then
            fail "scripts/$script is a copy of rust-fs-core's; delete it and run it as scripts/core.sh ${script%.sh}"
        fi
    done

    # 2. One bootstrap.
    if [ ! -f "$CALLER/scripts/core.sh" ]; then
        fail "scripts/core.sh is missing; copy rust-fs-core's scripts/core.sh unchanged"
    elif ! cmp -s "$CALLER/scripts/core.sh" "$CORE/scripts/core.sh"; then
        fail "scripts/core.sh differs from rust-fs-core's; copy it again, unchanged"
    fi

    # 3. Nothing calls a copy. Only files a run reads: workflows, chores.yml,
    #    and the repository's own scripts.
    targets=()
    for path in "$CALLER"/.github/workflows/*.yml "$CALLER"/.github/workflows/*.yaml \
                "$CALLER"/chores.yml "$CALLER"/scripts/*.sh; do
        [ -f "$path" ] && targets+=("$path")
    done
    if [ "${#targets[@]}" -gt 0 ]; then
        hits="$(grep -nE 'scripts/(test-floor|semver-check|guest-rust-toolchain)\.sh' "${targets[@]}" 2>/dev/null \
            | grep -vE '^\s*#|:[0-9]+:\s*#' || true)"
        if [ -n "$hits" ]; then
            fail "these run a local copy instead of scripts/core.sh:"$'\n'"${hits//$CALLER\//}"
        fi
    fi
fi

if [ "$fails" -gt 0 ]; then
    echo "family-check: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  family-check: no copy of a family script, and scripts/core.sh is rust-fs-core's"
