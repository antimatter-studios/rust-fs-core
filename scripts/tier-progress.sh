#!/usr/bin/env bash
# tier-progress.sh [TIER]
# tier-progress.sh --record TIER
#
# How far the tiers of the repository it is run from have got, one line each,
# from the records tier.sh leaves in tmp/logs. Run in place, as
# `bash ../rust-fs-core/scripts/tier.sh progress [TIER]`. It only reads, so it
# can be asked at any time and a tier never knows.
#
#   debug  running  build  12/68 crates  17%  0 failed  elapsed 9s  eta unknown
#   debug  running  test  412/490 tests  84%  0 failed  elapsed 2m10s  eta 25s (rate alone)
#   unit  running  412/750 lines  54%  0 failed  elapsed 2m10s  eta 1m50s
#
# A CARGO TIER is read from what cargo and libtest print into its log. It is in
# the build phase after a `Compiling` line, counted against the packages in the
# caller's Cargo.lock, and in the test phase after a `running N tests` line. The
# tests done, passing or quiet, are counted against the total of the binaries
# STARTED SO FAR, so that total grows as binaries start.
#
# THE ETA uses the last passing run's durations when tier.sh has recorded them in
# tmp/logs/TIER.history (`--record`): in the test phase, the time that run still
# had to go from this point, scaled by how this run compares so far; in the build
# phase, its duration less the time elapsed. Without that record it comes from
# the rate alone, and says so; a first build has none.
#
# ANY OTHER TIER is lines against the budget, and a budget has margin, so it reads
# low and the ETA reads long: a tier can end at 70%.
#
# The percentage stays at 99 until the tier has ended, and only then reads 100.
# `running` or `dead` comes from the recorded process, so it says `dead` when
# asked from another machine than the tier's, such as the host about a tier in a
# guest. A process killed outright leaves `dead`.
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

# What a log says, as `key=value` lines and one `binary SECONDS TESTS NAME` line
# for each test binary that has finished. POSIX awk: it runs under mawk and the
# BSD awk too. CI colours cargo's lines and Windows ends lines with CRLF.
scan() {
    awk '
        function tally() { return finished + current }
        { gsub(/\033\[[0-9;]*m/, ""); sub(/\r$/, "") }
        /^test .* \.\.\. FAILED|^.+ --- FAILED$/ { failed++ }
        /^ *Compiling [^ ]+ v[0-9]/ { compiled++; phase = "build"; next }
        /^ *Running .*\(.*\)$/ {
            name = $0; sub(/^ *Running /, "", name); sub(/ \([^(]*\)$/, "", name)
            file = $0; sub(/\)$/, "", file); sub(/^.*[\/\\(]/, "", file)
            sub(/\.exe$/, "", file); sub(/-[0-9a-f]+$/, "", file)
            binary = name " (" file ")"; next
        }
        /^ *Doc-tests / { binary = $0; sub(/^ */, "", binary); next }
        /^running [0-9]+ tests?$/ { phase = "test"; total += $2; size = $2; current = 0; open = 1; next }
        !open { next }
        /^test result:/ {
            ran = 0
            for (i = 2; i <= NF; i++)
                if ($i ~ /^(passed|failed|ignored|measured);$/) ran += $(i - 1)
            seconds = $0
            if (sub(/^.*finished in /, "", seconds) && binary != "") {
                sub(/s.*$/, "", seconds)
                printf "binary %s %d %s\n", seconds, ran, binary
            }
            finished += ran; current = 0; open = 0; binary = ""; next
        }
        /^test .* \.\.\. (ok|FAILED|ignored)/ { current++; next }
        / --- FAILED$/ { current++; next }
        /^[.iF]+( [0-9]+\/[0-9]+$|test |$)/ {
            # A full quiet row ends with the exact count; an unfinished one is counted.
            if (match($0, / [0-9]+\/[0-9]+$/)) {
                count = substr($0, RSTART + 1); sub(/\/.*$/, "", count); current = count + 0
            } else {
                match($0, /^[.iF]+/); current += RLENGTH
            }
        }
        END {
            printf "phase=%s\ncompiled=%d\ntotal=%d\ndone=%d\nfailed=%d\n", phase, compiled, total, tally(), failed
            if (open) printf "binary_now=%s\nsize_now=%d\ndone_now=%d\n", binary, size, current
        }' "$1"
}

# Where the last run was when it had got as far as this one: `EXPECTED WHOLE`, in
# seconds. $1 is its history, $2 this log as scan() read it.
reached() {
    printf '%s\n' "$2" | awk '
        function name_from(n,   s, i) { s = $0; for (i = 1; i < n; i++) sub(/^[^ ]+ /, "", s); return s }
        FNR == NR {
            if (/^duration=/) { whole = substr($0, 10) + 0 }
            else if (/^compiled=/) { crates = substr($0, 10) + 0 }
            else if ($1 == "binary") { key = name_from(4); secs[key] = $2; tests[key] = $3; testing += $2 }
            next
        }
        /^phase=/ { phase = substr($0, 7) }
        /^compiled=/ { compiled = substr($0, 10) + 0 }
        /^binary_now=/ { now = substr($0, 12) }
        /^done_now=/ { done_now = substr($0, 10) + 0 }
        $1 == "binary" { ran[name_from(4)] = 1 }
        END {
            build = whole - testing; if (build < 0) build = 0
            if (phase == "test") {
                at = build
                for (key in ran) at += secs[key]
                if (now in secs && tests[now] > 0) {
                    part = done_now / tests[now]; if (part > 1) part = 1
                    at += secs[now] * part
                }
            } else if (crates > 0) {
                part = compiled / crates; if (part > 1) part = 1
                at = build * part
            }
            printf "%d %d\n", at + 0.5, whole + 0.5
        }' "$1" -
}

value() { printf '%s\n' "$1" | sed -n "s/^$2=//p" | sed -n 1p; }

# What a tier that passed leaves for the next run's ETA.
record() {
    local meta="$LOGS/$1.meta" log="$LOGS/$1.log" history="$LOGS/$1.history" started ended seen
    started="$(field "$meta" started)"
    ended="$(field "$meta" ended)"
    case "$started:$ended" in *[!0-9:]* | :* | *:) return 1 ;; esac
    seen="$(scan "$log")" || return 1
    {
        printf 'duration=%s\ncompiled=%s\n' "$((ended - started))" "$(value "$seen" compiled)"
        printf '%s\n' "$seen" | grep '^binary '
        true
    } > "$history.new" && mv "$history.new" "$history"
}

report() {
    local meta="$1" name log history started max pid ended status now lines failed p state elapsed eta
    local seen phase amount unit crates at=0 whole=0 source=""
    name="$(basename "$meta" .meta)"
    log="$LOGS/$name.log"
    history="$LOGS/$name.history"
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
    seen=""
    if [ -f "$log" ]; then
        lines="$(wc -l < "$log" | tr -d ' ')"
        seen="$(scan "$log")"
    fi
    # A failed test is `test NAME ... FAILED`, or `NAME --- FAILED` under
    # libtest's --quiet. A binary's `test result: FAILED.` line is neither.
    failed="$(value "$seen" failed)"
    phase="$(value "$seen" phase)"
    case "$phase" in
        test)
            amount="$(value "$seen" 'done')"
            max="$(value "$seen" total)"
            unit=tests
            ;;
        build)
            amount="$(value "$seen" compiled)"
            crates="$(grep -c '^\[\[package\]\]' "$CALLER/Cargo.lock" 2>/dev/null)"
            max="${crates:-0}"
            unit=crates
            ;;
        *)
            amount="$lines"
            unit=lines
            ;;
    esac
    if [ "$max" -gt 0 ]; then p=$((amount * 100 / max)); else p=0; [ "$unit" = lines ] && p=99; fi
    # A running tier's history is its last passing run; an ended one's may be its own.
    if [ -z "$ended" ] && [ -f "$history" ]; then
        read -r at whole <<< "$(reached "$history" "$seen")"
        if [ "${whole:-0}" -gt 0 ]; then
            source=" (from last run)"
            p=$((at * 100 / whole))
        fi
    fi
    [ "$p" -le 99 ] || p=99
    if [ -n "$ended" ]; then
        p=100
        state="done, exit $status"
        elapsed=$((ended - started))
        eta=0s
    else
        elapsed=$((now - started))
        if ! kill -0 "$pid" 2>/dev/null; then
            state=dead
            eta=none
        else
            state=running
            # Lines say nothing about the last run, so such a tier goes by the clock.
            if [ -n "$source" ] && [ "$unit" = lines ]; then
                p=$((elapsed * 100 / whole))
                [ "$p" -le 99 ] || p=99
            fi
            if [ -n "$source" ]; then
                # No tests of this run to compare yet: what is left of the last one.
                if [ "$phase" = test ] && [ "$at" -gt 0 ]; then
                    eta=$(((whole - at) * elapsed / at))
                else
                    eta=$((whole - elapsed))
                fi
                [ "$eta" -ge 0 ] || eta=0
                eta="$(pretty "$eta")$source"
            elif [ "$phase" = build ]; then
                eta=unknown
            else
                if [ "$p" -gt 0 ]; then eta="$(pretty $((elapsed * (100 - p) / p)))"; else eta=unknown; fi
                if [ "$phase" = test ]; then
                    [ "$amount" -gt 0 ] && eta="$(pretty $((elapsed * (max - amount) / amount)))"
                    eta="$eta (rate alone)"
                fi
            fi
        fi
    fi
    printf '%s  %s  %s%s/%s %s  %s%%  %s failed  elapsed %s  eta %s\n' \
        "$name" "$state" "${phase:+$phase  }" "$amount" "$max" "$unit" "$p" "${failed:-0}" \
        "$(pretty "$elapsed")" "$eta"
}

if [ "${1:-}" = --record ]; then
    record "${2:?tier-progress: --record needs a tier}"
    exit
fi

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
