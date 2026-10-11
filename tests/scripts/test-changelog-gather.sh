#!/usr/bin/env bash
# changelog-gather.sh is the release step that turns changelog.d/ fragments
# into a dated CHANGELOG.md section and deletes them. A release's notes are
# that section (the changelog-notes action reads it at the tag), so the
# section has to come out complete and in a fixed order, the fragments have
# to be gone afterwards -- an entry in two places is the duplication the
# fragments exist to avoid -- and a refusal must leave everything as it was.
#
#   bash tests/scripts/test-changelog-gather.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/changelog-gather.sh"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/changelog-gather.sh is missing" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/changelog-gather-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT
R="$work/r"
g() { git -C "$R" -c user.name=t -c user.email=t@example.com "$@" >/dev/null 2>&1; }

# fresh UNRELEASED: a git repository with one release, and an empty
# Unreleased heading when UNRELEASED is yes.
fresh() {
    rm -rf "$R" && mkdir -p "$R/changelog.d"
    {
        printf '# Changelog\n\nNotable changes, newest first.\n\n'
        [ "$1" = yes ] && printf '## [Unreleased]\n\n'
        printf '## [0.1.0] — 2026-01-01\n\n### Added\n\n- **A start.**\n'
    } >"$R/CHANGELOG.md"
    printf '# changelog.d\n' >"$R/changelog.d/README.md"
    printf '### Fixed\n\n- **The b fix.**\n' >"$R/changelog.d/fix-b.md"
    printf '### Breaking\n\n- **A rename.**\n\n### Added\n\n- **The a feature.**\n  Over two lines.\n' >"$R/changelog.d/feat-a.md"
    printf '### Fixed\n\n- **The c fix.**\n\n- **A second c fix.**\n' >"$R/changelog.d/fix-c.md"
    g init -q -b main && g add -A && g commit -q -m base
}

gather() { FS_CORE_CALLER="$R" bash "$SCRIPT" "$@" 2>&1; }

expected='# Changelog

Notable changes, newest first.

## [0.2.0] — 2026-10-11

### Added

- **The a feature.**
  Over two lines.

### Fixed

- **The b fix.**

- **The c fix.**

- **A second c fix.**

### Breaking

- **A rename.**

## [0.1.0] — 2026-01-01

### Added

- **A start.**'

for unreleased in yes no; do
    fresh "$unreleased"
    out="$(gather 0.2.0 2026-10-11)"
    status=$?
    if [ "$status" -eq 0 ] && [ "$(cat "$R/CHANGELOG.md")" = "$expected" ]; then
        ok "Unreleased heading $unreleased: the section is written above the last release, headings in order"
    else
        fail "Unreleased heading $unreleased: status $status ($out); CHANGELOG.md is:
$(cat "$R/CHANGELOG.md")"
    fi
    left="$(cd "$R/changelog.d" && ls -A)"
    if [ "$left" = README.md ]; then
        ok "Unreleased heading $unreleased: only README.md is left in changelog.d/"
    else
        fail "Unreleased heading $unreleased: changelog.d/ still holds: $left"
    fi
    staged="$(git -C "$R" diff --cached --name-status | sort | tr '\n' ' ')"
    if [ "$staged" = "D	changelog.d/feat-a.md D	changelog.d/fix-b.md D	changelog.d/fix-c.md " ]; then
        ok "Unreleased heading $unreleased: the fragments' deletion is staged for the release commit"
    else
        fail "Unreleased heading $unreleased: staged: '$staged'"
    fi
done

# A leading v is dropped; with no date, today's.
fresh yes
gather v0.2.0 >/dev/null
if grep -qE "^## \[0\.2\.0\] — $(date -u +%F)$" "$R/CHANGELOG.md"; then
    ok "a leading v is dropped, and the date defaults to today"
else
    fail "v0.2.0 with no date: $(grep '^## ' "$R/CHANGELOG.md" | head -2 | tr '\n' '|')"
fi

# Refusals change nothing.
refused() {
    local what="$1"; shift
    local before after out status
    before="$(cd "$R" && cat CHANGELOG.md && ls -A changelog.d)"
    out="$(gather "$@")"
    status=$?
    after="$(cd "$R" && cat CHANGELOG.md && ls -A changelog.d)"
    if [ "$status" -ne 0 ] && [ "$before" = "$after" ]; then
        ok "$what: refused, nothing changed"
    else
        fail "$what: status $status, changed: $([ "$before" = "$after" ] && echo no || echo yes): $out"
    fi
}

fresh yes
refused "a version CHANGELOG.md already has" 0.1.0
refused "no version"
refused "a date that is not YYYY-MM-DD" 0.2.0 11/10/2026
printf 'Words before a heading.\n' >"$R/changelog.d/bad.md"
refused "a malformed fragment" 0.2.0
fresh yes
rm "$R/changelog.d/"fix-*.md "$R/changelog.d/feat-a.md"
refused "no fragments to gather" 0.2.0

if [ "$fails" -gt 0 ]; then
    echo "test-changelog-gather: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  changelog-gather moves every fragment into one dated section, and refuses without changing anything"
