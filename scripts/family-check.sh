#!/usr/bin/env bash
# family-check.sh -- the calling repository runs the family's scripts from
# rust-fs-core and keeps no copy of its own. Run it as
# `bash ../rust-fs-core/scripts/family-check.sh`; this file is the only copy.
#
# The family scripts were once copied into every repository and the copies
# drifted, a fix reaching one repository and none of the others. A rule that
# a copy must not come back is only a rule if something fails when one does,
# so every repository runs this in CI, and it checks two things:
#
#   1. NO COPY. No family script is committed in the caller: not the scripts
#      themselves, and not scripts/core.sh or scripts/tier.sh, which existed
#      only to find this crate and drifted like any other copy (#212).
#   2. NOTHING CALLS A COPY. No workflow, chores.yml or script in the caller
#      runs a bare scripts/NAME.sh of the family's; the calls run rust-fs-core's
#      scripts in place, from its checkout beside the caller
#      (../rust-fs-core/scripts/NAME.sh) at the version the caller pins.
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
CALLER="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
CALLER="$(cd "$CALLER" && pwd)"

fails=0
fail() { printf 'FAIL  family-check: %s\n' "$1"; fails=$((fails + 1)); }

if [ "$CALLER" != "$CORE" ]; then
    NAMES="output-budget test-floor semver-check guest-rust-toolchain ci-gate package-cli stage-siblings guest-rust-run tier core release-notes changelog-draft family-check agents-core-check"

    # 1. No copy, the bootstrap and the tier runner included: each existed
    #    only to find this crate, and a copy nothing updates drifts (#212).
    for name in $NAMES; do
        if [ -e "$CALLER/scripts/$name.sh" ] && [ -f "$CORE/scripts/$name.sh" ]; then
            fail "scripts/$name.sh is a copy of rust-fs-core's; delete it and run ../rust-fs-core/scripts/$name.sh in place"
        fi
    done

    # 2. Nothing calls a copy. Only files a run reads: workflows, chores.yml,
    #    and the repository's own scripts. A call in place names the script
    #    under rust-fs-core's checkout (../rust-fs-core/scripts/NAME.sh, or a
    #    staged sibling's path); a call to a bare scripts/NAME.sh is a copy's.
    targets=()
    for path in "$CALLER"/.github/workflows/*.yml "$CALLER"/.github/workflows/*.yaml \
                "$CALLER"/chores.yml "$CALLER"/scripts/*.sh; do
        [ -f "$path" ] && targets+=("$path")
    done
    if [ "${#targets[@]}" -gt 0 ]; then
        alternation="$(printf '%s' "$NAMES" | tr ' ' '|')"
        # Every reference to a family script, minus the ones that run core's
        # in place: through a rust-fs-core checkout, or $FS_CORE_ROOT.
        hits="$(grep -nHE "scripts/($alternation)\.sh" "${targets[@]}" 2>/dev/null \
            | grep -vE ':[0-9]+:[[:space:]]*#' \
            | while IFS= read -r line; do
                rest="$(printf '%s' "$line" | sed -E "s#(rust-fs-core|FS_CORE_ROOT\}?)/scripts/($alternation)\.sh##g")"
                printf '%s' "$rest" | grep -qE "scripts/($alternation)\.sh" && printf '%s\n' "$line"
              done || true)"
        if [ -n "$hits" ]; then
            fail "these run a local copy instead of ../rust-fs-core/scripts in place:"$'\n'"${hits//$CALLER\//}"
        fi
    fi
fi

if [ "$fails" -gt 0 ]; then
    echo "family-check: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  family-check: no copy of a family script, and every call runs rust-fs-core's in place"
