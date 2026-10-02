#!/usr/bin/env bash
# test-floor.sh TIER FLOOR -- require at least FLOOR executed Rust tests.
set -euo pipefail

[ $# -eq 2 ] || { echo "usage: test-floor.sh TIER FLOOR" >&2; exit 2; }
TIER="$1"
FLOOR="$2"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$REPO/tmp/logs/$TIER.log"

if [ ! -f "$LOG" ]; then
    echo "test-floor.sh: $LOG is missing -- the $TIER tier did not run." >&2
    exit 1
fi

# `|| true` because grep finding NOTHING exits 1, and under `pipefail` and
# `-e` that ended this script at the assignment -- still failing, but
# silently, in exactly the case it exists to name: zero tests ran (#185).
ran="$({ grep -aoE 'test result: ok\. [0-9]+ passed' "$LOG" || true; } | \
    awk '{ sum += $4 } END { print sum + 0 }')"
if [ "$ran" -lt "$FLOOR" ]; then
    echo "::error::only $ran tests executed in the $TIER tier, floor is $FLOOR"
    echo "test-floor.sh: the $TIER tier executed $ran tests; floor is $FLOOR." >&2
    exit 1
fi
printf '%s\n' "$TIER: $ran tests executed (floor $FLOOR)"
