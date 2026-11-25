#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
cat > "$TMP/bin/chore" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$1" >> "$PROVE_CALLS"
[ "$1" = lint ] && exit 7
exit 0
STUB
chmod +x "$TMP/bin/chore"
export PROVE_CALLS="$TMP/calls"
export PATH="$TMP/bin:$PATH"
cd "$TMP"
set +e
output="$(bash "$ROOT/scripts/prove.sh" lint test:debug coverage 2>&1)"
status=$?
set -e
[ "$status" -eq 1 ] || { echo "expected aggregate failure, got $status"; exit 1; }
[ "$(cat "$PROVE_CALLS")" = $'lint\ntest:debug\ncoverage' ] || { echo "tasks did not all run in order"; exit 1; }
[[ "$output" == *"FAIL lint (exit 7)"* ]] || { echo "lint failure missing from summary"; exit 1; }
[[ "$output" == *"PASS test:debug"* && "$output" == *"PASS coverage"* ]] || { echo "later tiers missing from summary"; exit 1; }
echo "prove continues after failures and reports every task"
