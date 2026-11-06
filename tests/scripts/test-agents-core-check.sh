#!/usr/bin/env bash
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/agents-core-check.sh"
SANDBOX="$(mktemp -d "${TMPDIR:-/tmp}/agents-core-check.XXXXXX")" || exit 1
trap 'rm -r "$SANDBOX"' EXIT

fails=0
ok() { printf 'ok    %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1" >&2; fails=$((fails + 1)); }

expect() {
    local what="$1" want="$2" message="$3" output status
    output="$(FS_CORE_CALLER="$SANDBOX" bash "$SCRIPT" 2>&1)"
    status=$?
    if [ "$status" -eq "$want" ] && printf '%s\n' "$output" | grep -qF "$message"; then
        ok "$what"
    else
        fail "$what: expected exit $want and '$message'; got exit $status: $output"
    fi
}

cp "$REPO/AGENTS.md" "$SANDBOX/AGENTS.md" || exit 1
expect "the canonical guide passes" 0 "carries agent-core v10, unmodified"

sed 's/^## Claiming work$/## Claiming work /' "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "modified shared content is refused" 1 "the agent-core block does not match the canonical copy"

sed 's/agent-core v10/agent-core v9/g' "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "stale markers are refused" 1 "AGENTS.md has no agent-core BEGIN marker"

sed '/BEGIN SHARED BLOCK/d' "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "a missing BEGIN marker is refused" 1 "AGENTS.md has no agent-core BEGIN marker"

sed '/END SHARED BLOCK/d' "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "a missing END marker is refused" 1 "AGENTS.md has no agent-core END marker"

sed 's/sha256:[0-9a-f]*/sha256:0000000000000000000000000000000000000000000000000000000000000000/' \
    "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "a mismatched declared digest is refused" 1 "the BEGIN marker declares sha256:"

awk '/BEGIN SHARED BLOCK/ { print; inside = 1; next } /END SHARED BLOCK/ { inside = 0 } !inside' \
    "$REPO/AGENTS.md" > "$SANDBOX/AGENTS.md"
expect "an empty shared block is refused" 1 "the agent-core block is empty"

mv "$SANDBOX/AGENTS.md" "$SANDBOX/guide.hidden" || exit 1
expect "an absent guide is refused" 1 "AGENTS.md is missing"

[ "$fails" -eq 0 ] || exit 1
echo "test-agents-core-check: all checks passed"
