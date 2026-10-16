#!/usr/bin/env bash
# features-check.sh is what `chore lint` runs to keep unreleased features-page
# rows in features.d/ fragments and out of docs/features.md. A fragment it
# passes must be one a release can apply, so each refusal here is a fragment
# that would otherwise fail features-gather.sh in the release commit -- or two
# pull requests that would otherwise land contradictory rows.
#
#   bash tests/scripts/test-features-check.sh
# shellcheck disable=SC2016  # backticks are table text, not expansions
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/features-check.sh"
NAME="$(basename "${BASH_SOURCE[0]}" .sh)"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/features-check.sh is missing" >&2; echo "$NAME: 1 check(s) failed" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/features-check-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT
R="$work/r"

page='# Features

Work merged after the last release is **Unreleased (#N)** until the next one.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Superblock | Supported | 0.1.0 | | `a.rs` |
| Inodes | Supported | 0.1.0 | | `b.rs` |

## Writing

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Overwrite in place | Supported | 0.3.0 | | `w.rs` |'

# fresh: a repository with a released features page and an empty features.d/.
fresh() {
    rm -rf "$R" && mkdir -p "$R/docs" "$R/features.d"
    printf '%s\n' "$page" >"$R/docs/features.md"
    printf '# features.d\n\nOne fragment per pull request.\n' >"$R/features.d/README.md"
}
frag() { printf '%b' "$2" >"$R/features.d/$1"; }

# expect pass|fail DESCRIPTION [TEXT]: run against $R; a refusal must say TEXT.
expect() {
    local want="$1" what="$2" must="${3:-}" out status
    out="$(FS_CORE_CALLER="$R" bash "$SCRIPT" 2>&1)"
    status=$?
    if [ "$want" = pass ] && [ "$status" -eq 0 ]; then
        ok "$what: passes"
    elif [ "$want" = fail ] && [ "$status" -ne 0 ] && printf '%s' "$out" | grep -q -F -- "$must"; then
        ok "$what: refused"
    else
        fail "$what: wanted it to $want${must:+ saying '$must'}, status $status: $out"
    fi
}

fresh
expect pass "no fragments yet"
frag feat-a.md '## Writing\n\n| Truncate | Experimental | Unreleased (#11) | | `t.rs` |\n'
expect pass "a fragment adding a row"
frag feat-b.md '## Reading\n\n| Inodes | Supported | 0.1.0; v3 cores Unreleased (#12) | | `b.rs` |\n\nrename: Superblock\n| Superblock and its copies | Supported | Unreleased (#12) | | `a.rs` |\n\n## Writing\n\nremove: Overwrite in place\n'
expect pass "a second fragment replacing, renaming and removing in two sections"
touch "$R/features.d/.gitkeep"
expect pass "a .gitkeep beside the fragments"

rm -rf "$R"
mkdir -p "$R"
expect pass "a repository with no docs/features.md"
mkdir -p "$R/features.d" && printf '# features.d\n' >"$R/features.d/README.md"
expect pass "no docs/features.md, and a features.d/ with only its README"
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "fragments with no docs/features.md to apply them to" "there is no docs/features.md"

fresh
rm -rf "$R/features.d"
expect fail "a features page and no features.d/" "there is no features.d/"

fresh
sed -i.bak 's/^| Inodes | Supported | 0.1.0 |/| Inodes | Supported | 0.1.0; more Unreleased (#9) |/' "$R/docs/features.md" && rm -f "$R/docs/features.md.bak"
expect fail "an Unreleased cell written into the page" "docs/features.md:10: \`Inodes\` says Unreleased under Since"

fresh
frag feat-a.md '## Mounting\n\n| Remount | Experimental | Unreleased (#1) | | `m.rs` |\n'
expect fail "a section the page does not have" "has no section \`## Mounting\`"
fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | `b.rs` |\n'
expect fail "a row with fewer cells than the table has columns" "the row has 4 cell(s), and \`## Reading\` has 5 columns"
fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` | extra |\n'
expect fail "a row with more cells than the table has columns" "the row has 6 cell(s)"
fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
frag feat-b.md '## Reading\n\n| Inodes | Partial | Unreleased (#2) | | `b.rs` |\n'
expect fail "two fragments writing the same key" "features.d/feat-a.md:3 and features.d/feat-b.md:3 both write \`Inodes\` in \`## Reading\`"
fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n| Inodes | Partial | Unreleased (#1) | | `b.rs` |\n'
expect fail "one fragment writing a key twice" "features.d/feat-a.md:3 and features.d/feat-a.md:4 both write"
fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
frag feat-b.md '## Writing\n\n| Inodes | Supported | Unreleased (#2) | | `i.rs` |\n'
expect pass "the same key in two different sections"
fresh
frag fix-a.md '## Reading\n\nremove: Never there\n'
expect fail "removing a key the section does not hold" "there is no row \`Never there\` in \`## Reading\` to remove"
fresh
frag fix-a.md '## Reading\n\nremove: inodes\n'
expect fail "removing by a key that differs only in case" "there is no row \`inodes\`"
fresh
frag fix-a.md '## Reading\n\nrename: Inodes\n\n## Writing\n\n| Truncate | Experimental | Unreleased (#11) | | `t.rs` |\n'
expect fail "a rename not followed by its row" "\`rename: Inodes\` is not followed by the row it becomes"
fresh
frag fix-a.md '## Reading\n\nremove:\n'
expect fail "a remove naming no key" "\`remove:\` names no key"

fresh; frag x.md 'Some words first.\n\n## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "text before the first heading" "text before the first \`## Section\` heading"
fresh; frag x.md '## Reading\n\n'
expect fail "a heading with no rows" "the section \"Reading\" has no rows"
fresh; frag x.md '## Reading\n\nA sentence where a row belongs.\n'
expect fail "prose where a row belongs" "not a table row"
fresh; frag x.md '### Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "a level-3 heading" "only \`## Section\` headings belong in a fragment"
fresh; frag x.md '## Reading\n\n|---|---|---|---|---|\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "a separator row" "a separator row"
fresh; frag x.md '## Reading\n\n| Feature | State | Since | Tracking | Checked by |\n'
expect fail "the header row" "the header row"
fresh; frag x.md '## Reading\n\n| | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "a row with no key" "the first cell is empty"
fresh; frag x.md ''
expect fail "an empty fragment" "the fragment is empty"
fresh; frag notes.txt '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
expect fail "a file that is not a fragment" "features.d/notes.txt is not a fragment"

# The names are registered as family scripts: a caller that commits a copy of
# either is refused, like any other family script.
for script in features-check features-gather; do
    caller="$work/caller-$script"
    mkdir -p "$caller/scripts"
    printf 'echo mine\n' >"$caller/scripts/$script.sh"
    out="$(FS_CORE_CALLER="$caller" bash "$REPO/scripts/family-check.sh" 2>&1)"
    status=$?
    if [ "$status" -ne 0 ] && printf '%s' "$out" | grep -q -F "scripts/$script.sh is a copy of rust-fs-core's"; then
        ok "family-check refuses a committed copy of $script.sh"
    else
        fail "family-check let a copy of $script.sh through: status $status: $out"
    fi
done

if [ "$fails" -eq 0 ]; then
    echo "$NAME: all checks passed"
else
    echo "$NAME: $fails check(s) failed" >&2
fi
exit $(( fails > 0 ))
