#!/usr/bin/env bash
# changelog-check.sh is what `chore lint` runs to keep unreleased entries in
# changelog.d/ fragments and out of CHANGELOG.md. A fragment it passes must be
# one a release can gather, and anything it lets through has to be something
# changelog-gather.sh handles, so each refusal here is a fragment that would
# otherwise break a release.
#
#   bash tests/scripts/test-changelog-check.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/changelog-check.sh"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/changelog-check.sh is missing" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/changelog-check-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# fresh: a repository with a released CHANGELOG and an empty changelog.d/.
fresh() {
    rm -rf "$work/r" && mkdir -p "$work/r/changelog.d"
    printf '# Changelog\n\n## [0.1.0] — 2026-01-01\n\n### Added\n\n- **A start.**\n' >"$work/r/CHANGELOG.md"
    printf '# changelog.d\n\nOne fragment per pull request.\n' >"$work/r/changelog.d/README.md"
}
frag() { printf '%b' "$2" >"$work/r/changelog.d/$1"; }

# expect pass|fail DESCRIPTION: run against $work/r.
expect() {
    local want="$1" what="$2" out status
    out="$(FS_CORE_CALLER="$work/r" bash "$SCRIPT" 2>&1)"
    status=$?
    if { [ "$want" = pass ] && [ "$status" -eq 0 ]; } || { [ "$want" = fail ] && [ "$status" -ne 0 ]; }; then
        if [ "$want" = pass ]; then ok "$what: passes"; else ok "$what: refused"; fi
    else
        fail "$what: wanted it to $want, status $status: $out"
    fi
}

fresh
expect pass "no fragments yet"
frag fix-a.md '### Fixed\n\n- **A fix.** It is fixed.\n'
expect pass "one fragment"
frag feat-b.md '### Added\n\n- **A feature.**\n  Continued on a second line.\n\n### Breaking\n\n- **A rename.**\n'
expect pass "a fragment with two sections, one outside Keep a Changelog"
touch "$work/r/changelog.d/.gitkeep"
expect pass "a .gitkeep beside the fragments"

fresh; frag x.md 'Some words first.\n\n### Fixed\n\n- **A fix.**\n'
expect fail "text before the first heading"
fresh; frag x.md '### Fixed\n\n'
expect fail "a heading with no entries"
fresh; frag x.md '### Fixed\n\n- **A fix.**\n\n### Added\n\nWords but no entry.\n'
expect fail "a second heading with no entries"
fresh; frag x.md '## [0.2.0]\n\n### Fixed\n\n- **A fix.**\n'
expect fail "a version heading inside a fragment"
fresh; frag x.md '# Title\n\n### Fixed\n\n- **A fix.**\n'
expect fail "a level-1 heading"
fresh; frag x.md ''
expect fail "an empty fragment"
fresh; frag x.md '###\n\n- **A fix.**\n'
expect fail "a heading with no name"
fresh; frag notes.txt '### Fixed\n\n- **A fix.**\n'
expect fail "a file that is not a fragment"

fresh
printf '# Changelog\n\n## [Unreleased]\n\n## [0.1.0] — 2026-01-01\n\n- **A start.**\n' >"$work/r/CHANGELOG.md"
expect pass "an empty Unreleased heading left in CHANGELOG.md"
printf '# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- **Written straight in.**\n\n## [0.1.0] — 2026-01-01\n\n- **A start.**\n' >"$work/r/CHANGELOG.md"
expect fail "an entry written into CHANGELOG.md's Unreleased section"

printf '# Changelog\n\n## [Unreleased]\n\n[Unreleased]: https://example.com/r/compare/v0.1.0...HEAD\n' >"$work/r/CHANGELOG.md"
expect pass "an empty Unreleased heading with only link definitions after it"

rm -rf "$work/r/changelog.d"
expect fail "no changelog.d/ at all"

if [ "$fails" -gt 0 ]; then
    echo "test-changelog-check: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  changelog-check passes only fragments a release can gather, and no entries in CHANGELOG.md"
