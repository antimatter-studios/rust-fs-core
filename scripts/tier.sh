#!/usr/bin/env bash
# tier.sh [--refuse-skips] LABEL LOG-NAME MAX-LINES MAX-BYTES -- COMMAND [ARG...]
#
# The family's tier runner: one quiet, budgeted test tier around
# output-budget.sh, writing tmp/logs/LOG-NAME.log in the repository it is run
# from. Every repository runs it in place, as
# `bash ../rust-fs-core/scripts/tier.sh ...`; none keeps a copy (#212).
set -euo pipefail

# Run in place from the caller's checkout of this crate (#212): the log is the
# caller's, and the budget wrapper is the one beside this file.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CALLER="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

# --refuse-skips: a run that printed `SKIP:` lines fails (exit 66) even when
# the command passed. A skipped test reads exactly like a passing one, and
# three repositories had grown this check in their own copies of this file.
REFUSE_SKIPS=0
if [ "${1:-}" = "--refuse-skips" ]; then REFUSE_SKIPS=1; shift; fi

[ $# -ge 5 ] || { echo "tier.sh: usage: tier.sh [--refuse-skips] LABEL LOG MAX-LINES MAX-BYTES -- CMD..." >&2; exit 2; }
LABEL="$1"
LOG_NAME="$2"
MAX_LINES="$3"
MAX_BYTES="$4"
shift 4
[ "${1:-}" = "--" ] && shift
[ $# -gt 0 ] || { echo "tier.sh: no command" >&2; exit 2; }

case " ${CLI_ARGS:-} " in
    *" --verbose "*|*" -v "*) export OUTPUT_BUDGET_VERBOSE=1 ;;
esac
[ "${AM_FS_CORE_VERBOSE:-0}" = 1 ] && export OUTPUT_BUDGET_VERBOSE=1

LOG="$CALLER/tmp/logs/$LOG_NAME.log"
mkdir -p "$(dirname "$LOG")"
set +e
bash "$HERE/output-budget.sh" \
    --log "$LOG" \
    --max-lines "$MAX_LINES" \
    --max-bytes "$MAX_BYTES" \
    --label "$LABEL" \
    -- "$@"
status=$?
set -e

if [ "$REFUSE_SKIPS" = 1 ] && [ -f "$LOG" ]; then
    skips=$(grep -ac '^SKIP:' "$LOG" || true)
    if [ "${skips:-0}" -gt 0 ]; then
        echo "::error::$LABEL: $skips test(s) printed SKIP and were counted as passing. A skipped test is not a passing test; see $LOG" >&2
        grep -a '^SKIP:' "$LOG" | sed 's/^/    /' >&2
        exit 66
    fi
fi
exit "$status"
