#!/usr/bin/env bash
# test-floor.sh [--refuse-ignored] TIER FLOOR -- the tier ran at least FLOOR
# tests. test-floor.sh --targets FILE TIER -- each target met its own floor.
# Run it as `scripts/core.sh test-floor ...` from any repository in the
# family; this file is the only copy.
#
# THE FAILURE A BUDGET CANNOT SEE. scripts/output-budget.sh fails a tier that
# PRINTS too much; nothing fails a tier that prints almost nothing because it
# RAN almost nothing. `cargo test` exits 0 on "0 passed; 0 failed", so a filter
# that selected nothing, a build that produced no test binary and a suite that
# ran in full all report the same green. A count is the only thing that sees
# an absence.
#
# IT READS THE CALLER'S LOG: $FS_CORE_CALLER/tmp/logs/TIER.log, written there
# by the caller's scripts/tier.sh. scripts/core.sh sets FS_CORE_CALLER; run
# directly, the repository is the one this file is in.
#
# WHAT COUNTS AS EXECUTED:
#   * cargo's `test result: ok. N passed`, summed over every test binary;
#   * cargo-semver-checks' `Checked [..] N checks:`, so a semver tier has a
#     floor on the lints it ran. CI sets CARGO_TERM_COLOR=always and that line
#     arrives wrapped in escapes, so they are stripped before counting.
#
# --targets FILE TIER checks PER-TARGET floors instead: FILE (relative to the
# caller) holds `<target> <floor>` lines, and each integration target's own
# `test result:` count in the tier log must meet its floor. A total is the
# weaker check -- a suite that empties from the inside hides inside a total
# that other suites kept above the floor -- and a target missing from the log
# counts as zero, so a suite that stopped being built fails here too. cargo
# colours its `Running` lines under CARGO_TERM_COLOR=always, so escapes come
# off before the pairing, or every target reads as zero.
#
# --refuse-ignored FAILS A TIER THAT IGNORED ANY TEST. A skipped test reads
# exactly like a passing one; a repository that runs every test in a tier
# asks for this so that an `#[ignore]` cannot quietly take one out.
#
# A COUNT ANSWERS "DID ANYTHING RUN", NOT "DID IT CHECK ANYTHING". A test that
# returns early because a tool is missing still counts as passed; the answer to
# that is a test that fails rather than returns, not a bigger number here.
set -euo pipefail

TEST_FLOOR_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-test-floor %s\n' "$TEST_FLOOR_API_VERSION"
    exit 0
fi

usage() {
    echo "usage: test-floor.sh [--refuse-ignored] TIER FLOOR" >&2
    echo "       test-floor.sh --targets FLOORS-FILE TIER" >&2
    exit 2
}
REPO="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

refuse_ignored=0
targets=""
case "${1:-}" in
    --refuse-ignored) refuse_ignored=1; shift ;;
    --targets) [ $# -eq 3 ] || usage; targets="$2"; shift 2 ;;
esac
if [ -n "$targets" ]; then
    [ $# -eq 1 ] || usage
    TIER="$1"
else
    [ $# -eq 2 ] || usage
    TIER="$1"
    FLOOR="$2"
fi
LOG="$REPO/tmp/logs/$TIER.log"

if [ ! -f "$LOG" ]; then
    echo "test-floor.sh: $LOG is missing -- the $TIER tier did not run." >&2
    exit 1
fi

if [ -n "$targets" ]; then
    case "$targets" in /*) floors="$targets" ;; *) floors="$REPO/$targets" ;; esac
    [ -f "$floors" ] || { echo "test-floor.sh: no floors file at $floors" >&2; exit 2; }
    # cargo prints `Running tests/<name>.rs (...)` or `Running unittests
    # src/lib.rs (...)` before each binary, and libtest prints `test result:
    # ok. N passed` after it; pairing the two gives a count per target.
    counts="$(awk '
        { gsub(/\033\[[0-9;]*[a-zA-Z]/, "") }
        /^[[:space:]]*Running unittests/ { target = "lib"; next }
        /^[[:space:]]*Running tests\// {
            target = $2; sub(/^tests\//, "", target); sub(/\.rs$/, "", target); next
        }
        /^test result: ok\. [0-9]+ passed/ { if (target != "") seen[target] += $4 }
        END { for (t in seen) printf "%s %d\n", t, seen[t] }
    ' "$LOG")"
    bad=0
    checked=0
    while read -r target floor; do
        case "$target" in ''|\#*) continue ;; esac
        case "$floor" in ''|*[!0-9]*)
            echo "test-floor.sh: $targets: '$target' has a non-numeric floor '${floor:-(none)}'" >&2
            exit 2 ;;
        esac
        executed="$(printf '%s\n' "$counts" | awk -v t="$target" '$1 == t { print $2 }')"
        executed="${executed:-0}"
        checked=$((checked + 1))
        if [ "$executed" -lt "$floor" ]; then
            echo "::error::$target executed $executed tests, floor is $floor -- a target that executes fewer than its floor emptied from the inside rather than passed. If it legitimately shrank, lower its floor in $targets in the same commit."
            bad=1
        fi
    done < "$floors"
    [ "$checked" -gt 0 ] || { echo "::error::$targets named no targets, so this checked nothing"; exit 1; }
    [ "$bad" -eq 0 ] || exit 1
    echo "$TIER: $checked target(s) met their floors ($targets)"
    exit 0
fi

# Every count tolerates finding nothing: under `pipefail` a grep with no match
# exits 1, and `-e` used to end this script at the assignment -- failing, but
# silently, in exactly the case it exists to name.
plain="$(awk '{ gsub(/\033\[[0-9;]*m/, ""); print }' "$LOG")"

if [ "$refuse_ignored" -eq 1 ]; then
    ignored="$(printf '%s\n' "$plain" | awk '/test result: / {
        for (i = 1; i < NF; i++) if ($(i + 1) ~ /^ignored/) sum += $i
    } END { print sum + 0 }')"
    if [ "$ignored" -gt 0 ]; then
        echo "::error::$ignored tests ignored in the $TIER tier"
        echo "test-floor.sh: the $TIER tier ignored $ignored tests. A skipped test reads" >&2
        echo "               exactly like a passing one; nothing here skips." >&2
        { printf '%s\n' "$plain" | grep -E '^test .* \.\.\. ignored' || true; } | sed 's/^/                 /' >&2
        exit 1
    fi
fi

tests="$({ printf '%s\n' "$plain" | grep -aoE 'test result: ok\. [0-9]+ passed' || true; } \
    | awk '{ sum += $4 } END { print sum + 0 }')"
checks="$({ printf '%s\n' "$plain" | grep -aoE 'Checked \[ *[0-9.]+s\] [0-9]+ checks:' || true; } \
    | awk '{ sum += $(NF-1) } END { print sum + 0 }')"
ran=$(( tests + checks ))

if [ "$ran" -lt "$FLOOR" ]; then
    echo "::error::only $ran tests executed in the $TIER tier, floor is $FLOOR -- a run that executes fewer than that stopped early rather than passed"
    echo "test-floor.sh: the $TIER tier executed $ran tests; the floor is $FLOOR." >&2
    echo "               A tier that runs fewer tests than it used to has stopped" >&2
    echo "               early rather than passed. The whole run is in $LOG." >&2
    exit 1
fi
printf '%s\n' "$TIER: $ran tests executed (floor $FLOOR)"
