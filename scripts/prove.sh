#!/usr/bin/env bash
# Run chore tasks in order, retaining the complete result even when one fails.
set -uo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: prove.sh LINT-TASK TIER-TASK..." >&2
    exit 2
fi

results=()
failed=0
for task in "$@"; do
    case "$task" in
        -*|*[!a-zA-Z0-9:_-]*)
            echo "prove.sh: invalid chore task name: $task" >&2
            exit 2
            ;;
    esac
    printf '\n==> chore %s\n' "$task"
    chore "$task"
    status=$?
    log="tmp/logs/${task#test:}.log"
    [ "$task" = "coverage" ] && log="tmp/logs/coverage.log"
    detail=
    [ -f "$log" ] && detail=" — $log"
    if [ "$status" -eq 0 ]; then
        results+=("PASS $task$detail")
    else
        results+=("FAIL $task (exit $status)$detail")
        failed=1
    fi
done

printf '\n==> prove summary\n'
for result in "${results[@]}"; do
    printf '%s\n' "$result"
done
if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
    printf 'NOT COVERED: macOS-only and x86_64-only CI jobs\n'
fi
exit "$failed"
