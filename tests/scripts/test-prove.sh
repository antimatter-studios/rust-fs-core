#!/usr/bin/env bash
# scripts/prove.sh runs every task it is given, fails when any fails, and ends
# with a summary naming each task and its log (#431).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/tmp/logs"
cat > "$TMP/bin/chore" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$1" >> "$PROVE_CALLS"
echo "output of $1"
case "$1" in
    lint) echo "lint said no" >&2; exit 7 ;;
    check:semver) echo ok > tmp/logs/semver.log ;;
    # A tier that passes and prints more than its budget, through the real
    # tier runner, as ci.yml runs it.
    test:loud) exec bash "$PROVE_ROOT/scripts/tier.sh" loud loud 1 1000 -- printf 'a\nb\nc\n' ;;
esac
exit 0
STUB
# The host the summary is asked about.
cat > "$TMP/bin/uname" <<'STUB'
#!/usr/bin/env bash
case "$1" in
    -s) echo "$PROVE_HOST_OS" ;;
    -m) echo "$PROVE_HOST_ARCH" ;;
    *) exit 1 ;;
esac
STUB
chmod +x "$TMP/bin/chore" "$TMP/bin/uname"
export PROVE_CALLS="$TMP/calls" PROVE_ROOT="$ROOT" FS_CORE_CALLER="$TMP"
export PROVE_HOST_OS=Linux PROVE_HOST_ARCH=x86_64
export PATH="$TMP/bin:$PATH"
unset CLI_ARGS AM_FS_CORE_VERBOSE OUTPUT_BUDGET_VERBOSE
cd "$TMP"
# A log left by an earlier run: this run's test:debug writes none.
echo old > tmp/logs/debug.log
touch -t 202001010000 tmp/logs/debug.log

fail() { echo "$1"; printf '%s\n' "$output"; exit 1; }
has() { grep -Fqx -- "$1" <<<"$output" || fail "missing from the output: $1"; }
run() {
    set +e
    output="$(bash "$ROOT/scripts/prove.sh" "$@" 2>&1)"
    status=$?
    set -e
}

run lint test:debug check:semver test:loud coverage
[ "$status" -eq 1 ] || fail "expected aggregate failure, got $status"
[ "$(cat "$PROVE_CALLS")" = $'lint\ntest:debug\ncheck:semver\ntest:loud\ncoverage' ] || fail "tasks did not all run in order"
has "FAIL lint (exit 7) — output: tmp/logs/prove-lint.log"
grep -Fqx "lint said no" tmp/logs/prove-lint.log || fail "the task's log does not hold its output"
has "PASS test:debug — output: tmp/logs/prove-test-debug.log"
has "PASS check:semver — log: tmp/logs/semver.log, output: tmp/logs/prove-check-semver.log"
has "FAIL test:loud (exit 65) — log: tmp/logs/loud.log, output: tmp/logs/prove-test-loud.log"
has "PASS coverage — output: tmp/logs/prove-coverage.log"
case "$output" in *"NOT COVERED"*) fail "an x86_64 Linux host was told something is not covered" ;; esac

run --not-covered "test / macos-latest" --not-covered "test / windows-latest" test:debug coverage
[ "$status" -eq 0 ] || fail "expected a pass, got $status"
case "$output" in *"NOT COVERED"*) fail "an x86_64 Linux host was told something is not covered" ;; esac

PROVE_HOST_OS=Darwin PROVE_HOST_ARCH=arm64
run --not-covered "test / macos-latest" --not-covered "test / windows-latest" test:debug
[ "$status" -eq 0 ] || fail "expected a pass, got $status"
has "NOT COVERED: test / macos-latest"
has "NOT COVERED: test / windows-latest"
has "NOT COVERED: Linux behaviour, because these tasks ran natively on Darwin"
run test:debug
has "NOT COVERED: macOS-only and x86_64-only CI jobs"

: > "$PROVE_CALLS"
run lint '--bad'
[ "$status" -eq 2 ] || fail "a bad task name was not refused, got $status"
[ ! -s "$PROVE_CALLS" ] || fail "a task ran before the bad name was refused"
run --not-covered
[ "$status" -eq 2 ] || fail "--not-covered with no job was not refused, got $status"
output="$(bash "$ROOT/scripts/prove.sh" --version)"
[ "$output" = "rust-fs-core-prove 1" ] || fail "--version does not state the contract"
echo "prove runs every task, fails on any failure and names each log"
