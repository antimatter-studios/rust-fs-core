#!/usr/bin/env bash
# A tier can be asked how far it has got, from outside it: tier.sh leaves a record
# beside its log, tier-progress.sh reads the two, and what a tier prints and logs
# does not change.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TIER="$ROOT/scripts/tier.sh"
PROGRESS="$ROOT/scripts/tier-progress.sh"
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
contains() { case "$1" in *"$2"*) ok "$3" ;; *) bad "$3 (lacked '$2'; got: $1)" ;; esac; }
lacks() { case "$1" in *"$2"*) bad "$3 (held '$2'; got: $1)" ;; *) ok "$3" ;; esac; }
same() { if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (wanted '$2', got '$1')"; fi; }
matches() { if printf '%s\n' "$1" | grep -Eq "$2"; then ok "$3"; else bad "$3 (no match for '$2'; got: $1)"; fi; }

SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/tier-progress.XXXXXX")" || exit 1
trap 'rm -rf "$SANDBOX"' EXIT
export FS_CORE_CALLER="$SANDBOX/repo"
LOGS="$FS_CORE_CALLER/tmp/logs"
mkdir -p "$LOGS"

progress() { bash "$PROGRESS" "$@"; }
wait_for() {
    local tries=0
    until [ -e "$1" ] || [ "$tries" -ge 600 ]; do sleep 0.1; tries=$((tries + 1)); done
    [ -e "$1" ]
}

# --- a tier that has not ended -------------------------------------------------

cat > "$SANDBOX/work.sh" <<'WORK'
printf 'line %s\n' 1 2 3
echo 'test first ... FAILED'
echo 'test second ... ok'
echo 'FAILED appears here and is not a test'
echo 'failures: test third FAILED in prose'
echo 'test fourth ... FAILED'
: > "$SANDBOX/reached"
until [ -e "$SANDBOX/release" ]; do sleep 0.1; done
WORK
export SANDBOX
bash "$TIER" unit unit 16 100000 -- bash "$SANDBOX/work.sh" > "$SANDBOX/verdict" 2>&1 &
tier=$!
wait_for "$SANDBOX/reached" || bad "the tier under test never reached its gate"
meta="$(cat "$LOGS/unit.meta" 2>/dev/null)"
contains "$meta" "started=" "the record says when the tier started"
contains "$meta" "max_lines=16" "and its line budget"
contains "$meta" "pid=$tier" "and which process runs it"
line="$(progress)"
contains "$line" "unit" "progress names the tier"
contains "$line" "running" "it says a live tier is running"
contains "$line" "8/16" "with its lines so far against its budget"
contains "$line" "50%" "and that as a percentage"
contains "$line" "2 failed" "counting only lines that start 'test' and end FAILED"
matches "$line" "elapsed [0-9]" "and how long it has run"
matches "$line" "eta [0-9]" "and when it should end"
: > "$SANDBOX/release"
wait "$tier"
check_status=$?
same "$check_status" 0 "the tier keeps its status"

# --- a tier that has ended -----------------------------------------------------

line="$(progress)"
contains "$line" "done" "an ended tier says so"
contains "$line" "100%" "and it alone reads 100%"
lacks "$line" "running" "and it is not running"
verdict="$(cat "$SANDBOX/verdict")"
same "$(printf '%s\n' "$verdict" | wc -l | tr -d ' ')" 1 "a tier still prints one verdict line"
lacks "$(cat "$LOGS/unit.log")" "started=" "and nothing of the record reaches its log"
same "$(wc -l < "$LOGS/unit.log" | tr -d ' ')" 8 "its log holds only what the command printed"

bash "$TIER" failing failing 10 100000 -- bash -c 'echo one; exit 3' > /dev/null 2>&1
contains "$(progress)" "failing" "a tier that failed is listed"
matches "$(progress | grep '^failing')" "done.*exit 3|exit 3.*done" "with the status it ended on"

# --- a tier nobody is running any more ------------------------------------------

rm -f "$SANDBOX/release" "$SANDBOX/reached"
bash "$TIER" killed killed 16 100000 -- bash "$SANDBOX/work.sh" > /dev/null 2>&1 &
tier=$!
wait_for "$SANDBOX/reached" || bad "the tier to be killed never reached its gate"
kill -KILL "$tier"
wait "$tier" 2>/dev/null
: > "$SANDBOX/release"
line="$(progress | grep '^killed')"
contains "$line" "dead" "a tier whose process has gone reads dead"
lacks "$line" "100%" "and not as finished"

# --- the numbers, from a record ---------------------------------------------------

rm -f "$LOGS"/*.meta "$LOGS"/*.log
record() { # record <name> <seconds ago> <max lines> <lines in the log>
    printf 'started=%s\nmax_lines=%s\npid=%s\n' "$(( $(date +%s) - $2 ))" "$3" "$$" > "$LOGS/$1.meta"
    : > "$LOGS/$1.log"
    local n=0
    while [ "$n" -lt "$4" ]; do echo x >> "$LOGS/$1.log"; n=$((n + 1)); done
}
record half 100 10 5
line="$(progress half)"
matches "$line" "50%" "half a budget reads 50%"
matches "$line" "elapsed 1m4[0-9]s" "after 100 seconds"
matches "$line" "eta 1m4[0-9]s" "and an ETA of elapsed*(100-p)/p"
record over 10 4 9
contains "$(progress over)" "99%" "a tier past its budget that has not ended reads 99%"
record empty 10 4 0
contains "$(progress empty)" "0%" "a tier with no lines yet reads 0%"
contains "$(progress empty)" "eta unknown" "and has no ETA to give"
same "$(progress half | wc -l | tr -d ' ')" 1 "a name asks for that tier alone"
same "$(progress | wc -l | tr -d ' ')" 3 "no name lists one line per tier"

progress nosuch > "$SANDBOX/out" 2>&1
same "$?" 1 "a tier with no record fails"
contains "$(cat "$SANDBOX/out")" "nosuch" "and names what it looked for"

# --- a tier that runs libtest with --quiet ----------------------------------------

# Such a log has a dot per passing test and `NAME --- FAILED` per failing one.
record quiet 10 10 0
printf '%s\n' \
    '....... 7/10' \
    'caching::tests::first --- FAILED' \
    '.' \
    'caching::tests::second --- FAILED' \
    'prose that quotes --- FAILED and goes on' \
    'test result: FAILED. 8 passed; 2 failed; 0 ignored; 0 measured' > "$LOGS/quiet.log"
contains "$(progress quiet)" "2 failed" "a quiet tier's failed tests are counted, and its summary line is not one"

rm -f "$LOGS"/*.meta "$LOGS"/*.log
out="$(progress 2>&1)"
same "$?" 0 "no tiers at all is not a failure"
contains "$out" "no tier" "and it says so"

# --- a cargo tier: its phase, and tests done against the total -------------------

printf '[[package]]\nname = "%s"\n\n' a b c d > "$FS_CORE_CALLER/Cargo.lock"
cargo_log() { # cargo_log <name> <seconds ago> <line>...
    local name="$1"
    record "$name" "$2" 10 0
    shift 2
    printf '%s\n' "$@" > "$LOGS/$name.log"
}
ran_a='     Running tests/a.rs (target/debug/deps/a-0123456789abcdef)'
ran_b='     Running tests/b.rs (target/debug/deps/b-fedcba9876543210)'
result_a='test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s'

# CI sets CARGO_TERM_COLOR=always, so cargo's own lines carry colour codes.
cargo_log building 10 \
    "$(printf '\033[1m\033[32m   Compiling\033[0m a v1.0.0')" \
    '   Compiling b v1.0.0'
line="$(progress building)"
contains "$line" "  build  " "a tier that is compiling is in the build phase"
contains "$line" "2/4 crates" "with crates compiled against those in Cargo.lock"
contains "$line" "50%" "as a percentage"
contains "$line" "eta unknown" "and a first run has no ETA while it builds"

cargo_log testing 60 \
    '   Compiling a v1.0.0' "$ran_a" 'running 3 tests' '...' "$result_a" \
    "$ran_b" 'running 100 tests' '..... 5/100' 'b::bad --- FAILED'
printf '...' >> "$LOGS/testing.log"
line="$(progress testing)"
contains "$line" "  test  " "a tier that is running tests is in the test phase"
contains "$line" "12/103 tests" "counting quiet rows, failures and the unfinished row"
contains "$line" "11%" "against the total known so far"
contains "$line" "1 failed" "with its failures"
matches "$line" 'eta 7m[0-9]+s \(rate alone\)' "and a first run says its ETA comes from the rate alone"

# libtest's default format, with the line endings Windows writes.
cargo_log listed 10 "$ran_a" 'running 4 tests'
printf 'test one ... ok\r\ntest two ... FAILED\r\ntest three ... ignored\r\n' >> "$LOGS/listed.log"
line="$(progress listed)"
contains "$line" "3/4 tests" "a tier that lists its tests is counted too"
contains "$line" "75%" "as a percentage"
contains "$line" "1 failed" "with its failure"

# --- the last run's durations ------------------------------------------------------

# 100s last time: 20s of build, 20s in a and 60s in b. This run is half way through
# b after 140s, where the last one had taken 70s: it is twice as slow, so the 30s
# that were left then are 60s now.
cargo_log again 140 "$ran_a" 'running 3 tests' '...' "$result_a" "$ran_b" 'running 10 tests'
printf '.....' >> "$LOGS/again.log"
printf '%s\n' 'duration=100' 'compiled=4' \
    'binary 20.00 3 tests/a.rs (a)' 'binary 60.00 10 tests/b.rs (b)' > "$LOGS/again.history"
line="$(progress again)"
contains "$line" "8/13 tests" "tests are still counted when there is a last run"
contains "$line" "70%" "the percentage is the last run's time so far over its whole"
matches "$line" 'eta 1m[0-3]s \(from last run\)' "and the ETA is what was left then, scaled by this run"

cargo_log rebuilding 30 '   Compiling a v1.0.0'
cp "$LOGS/again.history" "$LOGS/rebuilding.history"
line="$(progress rebuilding)"
matches "$line" 'eta 1m(10|[6-9])s \(from last run\)' "a build takes its ETA from the last run's duration"

# --- what a tier records when it ends ------------------------------------------------

cat > "$SANDBOX/cargo.sh" <<WORK
echo '   Compiling a v1.0.0'
echo '$ran_a'
echo 'running 2 tests'
echo '..'
echo 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.50s'
WORK
bash "$TIER" real real 100 100000 -- bash "$SANDBOX/cargo.sh" > "$SANDBOX/verdict" 2>&1
history="$(cat "$LOGS/real.history" 2>/dev/null)"
matches "$history" '^duration=[0-9]+$' "an ended tier records its duration"
contains "$history" "compiled=1" "how many crates it compiled"
contains "$history" "binary 1.50 2 tests/a.rs (a)" "and each test binary's duration"
same "$(wc -l < "$SANDBOX/verdict" | tr -d ' ')" 1 "it still prints one verdict line"
same "$(wc -l < "$LOGS/real.log" | tr -d ' ')" 5 "and its log holds only what the command printed"
bash "$TIER" real real 100 100000 -- bash -c 'exit 3' > /dev/null 2>&1
same "$(cat "$LOGS/real.history" 2>/dev/null)" "$history" "a failed run does not replace the last good one's record"

# --- asked through tier.sh ------------------------------------------------------------

out="$(bash "$TIER" progress again 2>&1)"
same "$?" 0 "tier.sh progress NAME succeeds"
contains "$out" "8/13 tests" "and prints that tier's line"
out="$(bash "$TIER" progress 2>&1)"
contains "$out" "building" "tier.sh progress lists every tier"
contains "$out" "testing" "one line each"

if [ "$fails" -gt 0 ]; then
    echo "test-tier-progress: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  a tier can be asked how far it has got, and prints and logs what it did before"
