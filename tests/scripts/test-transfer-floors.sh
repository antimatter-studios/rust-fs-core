#!/usr/bin/env bash
# scripts/transfer-floors.sh checks the common transfer floors and then the
# host OS's, as ci.yml does, and refuses a host that has no floors file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/repo/scripts"
cp "$ROOT/scripts/transfer-floors.sh" "$TMP/repo/scripts/"
cat > "$TMP/repo/scripts/core.sh" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FLOOR_CALLS"
[ "$3" != "${FAIL_FLOORS:-}" ]
STUB
cat > "$TMP/bin/uname" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$FAKE_UNAME"
STUB
chmod +x "$TMP/bin/uname"
export FLOOR_CALLS="$TMP/calls"
export PATH="$TMP/bin:$PATH"

fail() { echo "$*" >&2; exit 1; }

run() {
    : > "$FLOOR_CALLS"
    set +e
    output="$(FAKE_UNAME="$1" bash "$TMP/repo/scripts/transfer-floors.sh" "${@:2}" 2>&1)"
    status=$?
    set -e
}

common=.github/transfer-test-floors.txt
for host in Linux:linux Darwin:macos MINGW64_NT-10.0:windows MSYS_NT-10.0:windows; do
    os_file=".github/transfer-${host#*:}-test-floors.txt"
    [ -f "$ROOT/$os_file" ] || fail "$os_file does not exist"
    run "${host%:*}" coverage
    [ "$status" -eq 0 ] || fail "${host%:*}: exit $status: $output"
    wanted="test-floor --targets $common coverage"$'\n'"test-floor --targets $os_file coverage"
    [ "$(cat "$FLOOR_CALLS")" = "$wanted" ] || fail "${host%:*} checked: $(cat "$FLOOR_CALLS")"
done

for failing in "$common" .github/transfer-linux-test-floors.txt; do
    FAIL_FLOORS="$failing" run Linux debug
    [ "$status" -ne 0 ] || fail "a floor unmet in $failing passed"
done

run FreeBSD debug
[ "$status" -ne 0 ] || fail "a host with no per-OS floors passed"
[[ "$output" == *"no per-OS transfer floors for FreeBSD"* ]] || fail "unnamed host: $output"

run Linux
[ "$status" -eq 2 ] || fail "a missing tier exited $status, not 2"

echo "transfer-floors checks the common and the host's floors, and refuses an unknown host"
