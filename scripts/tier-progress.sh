#!/usr/bin/env bash
# tier-progress.sh [TIER]
#
# How far the tiers of the repository it is run from have got, one line each,
# from the records tier.sh leaves in tmp/logs. Run in place, as
# `bash ../rust-fs-core/scripts/tier-progress.sh`. It only reads, so it can be
# asked at any time and a tier never knows.
#
#   unit  running  412/750 lines  54%  0 failed  elapsed 2m10s  eta 1m50s
#
# THE PERCENTAGE IS LINES AGAINST THE BUDGET, and a budget has margin, so it reads
# low and the ETA reads long: a tier can end at 70%. It stays at 99 until the tier
# has ended, and only then reads 100. `running` or `dead` comes from the recorded
# process, so it says `dead` when asked from another machine than the tier's, such
# as the host about a tier in a guest. A process killed outright leaves `dead`.
set -uo pipefail

CALLER="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
LOGS="$CALLER/tmp/logs"

field() { sed -n "s/^$2=//p" "$1" | sed -n 1p; }

pretty() {
    if [ "$1" -lt 60 ]; then echo "$1s"
    elif [ "$1" -lt 3600 ]; then echo "$(($1 / 60))m$(($1 % 60))s"
    else echo "$(($1 / 3600))h$((($1 % 3600) / 60))m"
    fi
}

report() {
    local meta="$1" name log started max pid ended status now lines failed p state elapsed eta
    name="$(basename "$meta" .meta)"
    log="$LOGS/$name.log"
    started="$(field "$meta" started)"
    max="$(field "$meta" max_lines)"
    pid="$(field "$meta" pid)"
    ended="$(field "$meta" ended)"
    status="$(field "$meta" status)"
    case "$started:$max:$pid" in
        *[!0-9:]* | :* | *::* | *:) echo "$name  record unreadable: $meta"; return ;;
    esac
    now="$(date +%s)"
    lines=0
    failed=0
    if [ -f "$log" ]; then
        lines="$(wc -l < "$log" | tr -d ' ')"
        failed="$(grep -ac '^test .* FAILED' "$log")"
    fi
    if [ "$max" -gt 0 ]; then p=$((lines * 100 / max)); else p=99; fi
    [ "$p" -le 99 ] || p=99
    if [ -n "$ended" ]; then
        p=100
        state="done, exit $status"
        elapsed=$((ended - started))
        eta=0s
    else
        elapsed=$((now - started))
        if kill -0 "$pid" 2>/dev/null; then
            state=running
            if [ "$p" -gt 0 ]; then eta="$(pretty $((elapsed * (100 - p) / p)))"; else eta=unknown; fi
        else
            state=dead
            eta=none
        fi
    fi
    printf '%s  %s  %s/%s lines  %s%%  %s failed  elapsed %s  eta %s\n' \
        "$name" "$state" "$lines" "$max" "$p" "$failed" "$(pretty "$elapsed")" "$eta"
}

if [ $# -gt 0 ]; then
    [ -f "$LOGS/$1.meta" ] || { echo "tier-progress: no record of a tier '$1' in $LOGS" >&2; exit 1; }
    report "$LOGS/$1.meta"
    exit 0
fi

found=0
for meta in "$LOGS"/*.meta; do
    [ -f "$meta" ] || continue
    found=1
    report "$meta"
done
[ "$found" = 1 ] || echo "tier-progress: no tier has run in $CALLER"
exit 0
