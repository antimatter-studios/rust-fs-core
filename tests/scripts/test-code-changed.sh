#!/usr/bin/env bash
# code-changed.sh reads a change's paths, one per line, and prints `true`
# when any of them is not documentation, `false` when all of them are. Every
# repository's CI asks it whether a pull request needs the full pipeline, so
# its one way of being wrong that costs nothing is answering `true`: an empty
# list, an unknown file, a path it cannot classify all say `true`.
#
#   bash tests/scripts/test-code-changed.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/code-changed.sh"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/code-changed.sh is missing" >&2; exit 1; }

# expect WANT DESCRIPTION PATH...: the paths on stdin, WANT on stdout.
expect() {
    local want="$1" what="$2"; shift 2
    local got
    got="$(printf '%s\n' "$@" | bash "$SCRIPT")"
    if [ "$got" = "$want" ]; then
        ok "$what: $want"
    else
        fail "$what: wanted $want, got '$got' for: $*"
    fi
}

expect false "the README alone" README.md
expect false "AGENTS.md, CLAUDE.md and the CHANGELOG" AGENTS.md CLAUDE.md CHANGELOG.md
expect false "documentation in a docs directory, nested" docs/features.md docs/a/b/notes.md
expect false "a markdown file anywhere" src/README.md tests/NOTES.md
expect false "the licence" LICENSE LICENSE-APACHE
expect false "agent settings" .claude/settings.json .claude/skills/x/SKILL.md
expect true "source" src/lib.rs
expect true "source beside documentation" README.md src/lib.rs
expect true "a test" tests/oracle.rs
expect true "the manifest and lockfile" Cargo.toml Cargo.lock
expect true "a script that builds fixtures" scripts/guest-build-fixtures.sh
expect true "the task file" chores.yml
expect true "the workflow" .github/workflows/ci.yml
expect true "a file of a kind never seen" some/new-thing.bin
expect true "a markdown-looking name that is not markdown" docs/notes.md.in
got="$(printf '' | bash "$SCRIPT")"
if [ "$got" = "true" ]; then
    ok "no paths at all: true"
else
    fail "no paths at all: wanted true, got '$got'"
fi

if [ "$fails" -gt 0 ]; then
    echo "test-code-changed: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  code-changed says false only when every path is documentation"
