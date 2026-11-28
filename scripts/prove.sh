#!/usr/bin/env bash
# prove.sh [--not-covered JOB]... TASK... -- run chore tasks in order,
# going on after a failure so the whole picture shows, and end with a verdict
# naming each task and its logs. It fails when any task fails, with whatever
# status that task failed with: an over-budget tier fails here as it does in CI.
# Run in place, from the repository whose tasks it runs, as
# `bash ../rust-fs-core/scripts/prove.sh ...`; this file is the only copy.
#
# Every task's output is shown and kept in tmp/logs/prove-TASK.log. A task whose
# last name part is a tier (`test:debug` writes tmp/logs/debug.log) names that
# log too, but only when this run wrote it.
#
# --not-covered names a CI job that an arm64 Mac does not cover: a macOS-only,
# Windows-only or x86_64-only one. The summary lists them on such a host.
set -uo pipefail

PROVE_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-prove %s\n' "$PROVE_API_VERSION"
    exit 0
fi

usage() {
    echo "usage: prove.sh [--not-covered JOB]... TASK..." >&2
    exit 2
}

# A newline-separated string, not an array: macOS's bash 3.2 treats an empty
# array as unset under `set -u`.
not_covered=
while [ "${1:-}" = "--not-covered" ]; do
    [ "$#" -ge 2 ] || usage
    not_covered="$not_covered$2"$'\n'
    shift 2
done
[ "$#" -ge 1 ] || usage

# Every name is checked before the first task runs.
for task in "$@"; do
    case "$task" in
        -*|*[!a-zA-Z0-9:_-]*)
            echo "prove.sh: invalid chore task name: $task" >&2
            exit 2
            ;;
    esac
done

mkdir -p tmp/logs || exit 1
results=()
failed=0
for task in "$@"; do
    slug="${task//:/-}"
    output="tmp/logs/prove-$slug.log"
    started="tmp/logs/prove-$slug.started"
    : > "$started"
    printf '\n==> chore %s\n' "$task"
    chore "$task" 2>&1 | tee "$output"
    status=${PIPESTATUS[0]}
    tier_log="tmp/logs/${task##*:}.log"
    detail="output: $output"
    # A tier log older than this task is an earlier run's, and says nothing
    # about this one.
    if [ -f "$tier_log" ] && [ ! "$started" -nt "$tier_log" ]; then
        detail="log: $tier_log, $detail"
    fi
    rm -f "$started"
    if [ "$status" -eq 0 ]; then
        results+=("PASS $task — $detail")
    else
        results+=("FAIL $task (exit $status) — $detail")
        failed=1
    fi
done

printf '\n==> prove summary\n'
for result in "${results[@]}"; do
    printf '%s\n' "$result"
done
host="$(uname -s)"
if [ "$host" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
    if [ -n "$not_covered" ]; then
        while IFS= read -r job; do
            [ -n "$job" ] && printf 'NOT COVERED: %s\n' "$job"
        done <<<"$not_covered"
    else
        printf 'NOT COVERED: macOS-only and x86_64-only CI jobs\n'
    fi
fi
if [ "$host" != Linux ]; then
    printf 'NOT COVERED: Linux behaviour, because these tasks ran natively on %s\n' "$host"
fi
exit "$failed"
