#!/usr/bin/env bash
# tier.sh [--refuse-skips] [--refuse-ignored] LABEL LOG-NAME MAX-LINES MAX-BYTES -- COMMAND [ARG...]
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

# A skipped test reads exactly like a passing one, and three repositories had
# grown these checks in their own copies of this file. Each fails (exit 66) a
# tier whose command PASSED; a command that failed keeps its own status, which
# has the better story to tell.
#
#   --refuse-skips    the run printed `SKIP:` lines
#   --refuse-ignored  libtest's own summaries report ignored tests: `cargo
#                     test` prints `N ignored` and exits 0, so a tier that
#                     stopped running half of itself is otherwise a green line
REFUSE_SKIPS=0
REFUSE_IGNORED=0
while :; do
    case "${1:-}" in
        --refuse-skips) REFUSE_SKIPS=1; shift ;;
        --refuse-ignored) REFUSE_IGNORED=1; shift ;;
        *) break ;;
    esac
done

[ $# -ge 5 ] || { echo "tier.sh: usage: tier.sh [--refuse-skips] [--refuse-ignored] LABEL LOG MAX-LINES MAX-BYTES -- CMD..." >&2; exit 2; }
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

if [ "$status" -eq 0 ] && [ "$REFUSE_SKIPS" = 1 ] && [ -f "$LOG" ]; then
    skips=$(grep -ac '^SKIP:' "$LOG" || true)
    if [ "${skips:-0}" -gt 0 ]; then
        echo "::error::$LABEL: $skips test(s) printed SKIP and were counted as passing. A skipped test is not a passing test; see $LOG" >&2
        grep -a '^SKIP:' "$LOG" | sed 's/^/    /' >&2
        exit 66
    fi
fi
if [ "$status" -eq 0 ] && [ "$REFUSE_IGNORED" = 1 ] && [ -f "$LOG" ]; then
    # `test result: ok. 37 passed; 0 failed; 2 ignored; ...`, one line per test
    # binary. Anchored at the start of a line, so output quoting the phrase is
    # not read as a verdict, and the count is the field BEFORE the word, so a
    # libtest that reorders the summary does not silently read zero.
    ignored="$(awk '
        /^test result:/ {
            for (i = 1; i <= NF; i++) if ($i == "ignored;" || $i == "ignored") sum += $(i - 1)
        }
        END { print sum + 0 }' "$LOG")"
    if [ "$ignored" -gt 0 ]; then
        echo "::error::$LABEL: $ignored test(s) ignored. A skipped test is not a passing test; see $LOG" >&2
        exit 66
    fi
fi
exit "$status"
