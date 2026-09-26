#!/usr/bin/env bash
#
# Canonical output-budget wrapper for the rust-fs filesystem-driver family.
#
# Filesystem-driver siblings invoke this file directly through their pinned
# ../rust-fs-core checkout. They must not copy it. Each consumer remains
# responsible for its own task adapter, measured budgets, test floors and CI
# artifact paths; this script has no VM or harness dependency.
#
# output-budget.sh --log FILE [--max-lines N] [--max-bytes N] [--tail N]
#                  [--label TEXT] [--verbose] -- COMMAND [ARG...]
# output-budget.sh --version
#
# A successful run prints one verdict and keeps the full transcript in FILE.
# A failing run prints a verdict naming the status, the log and its size, and
# returns the command's own status. A successful command that breaches a
# budget returns 65. OUTPUT_BUDGET_VERBOSE=1 streams the transcript without
# lifting either budget.
#
# --tail N, or OUTPUT_BUDGET_FAIL_TAIL=N, prints the last N lines of the log
# when the command fails. It DEFAULTS TO 0: printing the tail is right for a
# person at a terminal and wrong for the reader who pays most, an agent that
# re-reads its whole transcript on every later step and so pays for those
# lines many times over -- and they are rarely the lines it needs, because the
# assertion is usually further up the log. One line naming the log lets it
# fetch exactly the part it wants, once.
#
# THE ENVIRONMENT VARIABLES ARE OUTPUT_BUDGET_*, NOT FLTH_*. This wrapper was
# written against fs-linux-test-harness, where they were FLTH_VERBOSE and
# FLTH_FAIL_TAIL; the names moved with the script when it became the family's
# canonical copy. A rename like that fails silently -- the old name is simply
# not read, and the run stays quiet -- so setting one is reported below rather
# than ignored. It is NOT honoured: a fallback would keep the old name alive
# in habits and documentation indefinitely.
set -uo pipefail

OUTPUT_BUDGET_API_VERSION=1

LOG=""
MAX_LINES=0
MAX_BYTES=0
TAIL="${OUTPUT_BUDGET_FAIL_TAIL:-0}"
LABEL=""
VERBOSE="${OUTPUT_BUDGET_VERBOSE:-0}"

superseded() {
    [ -n "${2:-}" ] || return 0
    echo "output-budget.sh: $1 is set and this script does not read it." >&2
    echo "                  The name is now $3." >&2
}
superseded FLTH_VERBOSE "${FLTH_VERBOSE:-}" OUTPUT_BUDGET_VERBOSE
superseded FLTH_FAIL_TAIL "${FLTH_FAIL_TAIL:-}" OUTPUT_BUDGET_FAIL_TAIL

while [ $# -gt 0 ]; do
    case "$1" in
        --log)       shift; LOG="${1:-}" ;;
        --max-lines) shift; MAX_LINES="${1:-0}" ;;
        --max-bytes) shift; MAX_BYTES="${1:-0}" ;;
        --tail)      shift; TAIL="${1:-0}" ;;
        --label)     shift; LABEL="${1:-}" ;;
        --verbose|-v) VERBOSE=1 ;;
        --version)
            printf 'rust-fs-core-output-budget %s\n' "$OUTPUT_BUDGET_API_VERSION"
            exit 0
            ;;
        --)          shift; break ;;
        -h|--help)
            awk '/^# output-budget\.sh/ { on = 1 } on && !/^#/ { exit } on' \
                "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "output-budget.sh: unknown argument '$1'" >&2; exit 2 ;;
    esac
    shift
done

[ $# -gt 0 ] || { echo "output-budget.sh: no command (use -- COMMAND ...)" >&2; exit 2; }
[ -n "$LOG" ] || { echo "output-budget.sh: --log is required" >&2; exit 2; }
[ -n "$LABEL" ] || LABEL="$1"

mkdir -p "$(dirname "$LOG")" || exit 2

if [ "$VERBOSE" = 1 ]; then
    "$@" 2>&1 | tee "$LOG"
    rc=${PIPESTATUS[0]}
else
    "$@" > "$LOG" 2>&1
    rc=$?
fi

lines=$(wc -l < "$LOG" | tr -d ' ')
bytes=$(wc -c < "$LOG" | tr -d ' ')

if [ "$rc" -ne 0 ]; then
    echo "$LABEL: FAILED (exit $rc) — $lines lines in $LOG" >&2
    # Verbose already streamed the run, so repeating its tail says nothing new.
    if [ "$VERBOSE" != 1 ] && [ "$TAIL" -gt 0 ]; then
        echo "--- last $TAIL lines of $LOG" >&2
        tail -n "$TAIL" "$LOG" >&2
    fi
    exit "$rc"
fi

over=""
[ "$MAX_LINES" -gt 0 ] && [ "$lines" -gt "$MAX_LINES" ] && \
    over="$lines lines (budget $MAX_LINES)"
if [ "$MAX_BYTES" -gt 0 ] && [ "$bytes" -gt "$MAX_BYTES" ]; then
    [ -n "$over" ] && over="$over, "
    over="$over$bytes bytes (budget $MAX_BYTES)"
fi

if [ -n "$over" ]; then
    echo "$LABEL: passed, but printed $over" >&2
    echo "             Quiet the run, or raise the measured budget deliberately." >&2
    echo "             Full output: $LOG" >&2
    exit 65
fi

printf '%s: ok (%s lines, %s bytes) — %s\n' "$LABEL" "$lines" "$bytes" "$LOG"
