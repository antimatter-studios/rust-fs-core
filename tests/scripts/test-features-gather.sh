#!/usr/bin/env bash
# features-gather.sh is the release step that applies features.d/ fragments
# to docs/features.md and deletes them. The page is what a reader trusts for
# what a release does, so every row a fragment adds, replaces, removes or
# renames has to land where the convention says, Unreleased has to become the
# version, the fragments have to be gone afterwards -- a row in two places is
# the duplication the fragments exist to avoid -- and a refusal must leave the
# page and the fragments exactly as they were: never a half-applied page.
#
#   bash tests/scripts/test-features-gather.sh
# shellcheck disable=SC2016,SC2329  # backticks are table text; the conditions run through check()
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/features-gather.sh"
NAME="$(basename "${BASH_SOURCE[0]}" .sh)"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/features-gather.sh is missing" >&2; echo "$NAME: 1 check(s) failed" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/features-gather-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT
R="$work/r"
P="$R/docs/features.md"
g() { git -C "$R" -c user.name=t -c user.email=t@example.com "$@" >/dev/null 2>&1; }

# The page every case starts from: prose that says Unreleased (which is not a
# cell, and stays), and two sections, the second with prose above its table.
page='# Features

Work merged after the last release is **Unreleased (#N)** until the next one.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Superblock | Supported | 0.1.0 | | `a.rs` |
| Inodes | Supported | 0.1.0 | | `b.rs` |
| Old name | Partial | 0.2.0 | #7 | `c.rs` |

## Writing

Prose before the table.

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Overwrite in place | Supported | 0.3.0 | | `w.rs` |
| Dropped thing | Experimental | 0.3.0 | | `d.rs` |

The end.'

# fresh: a git repository holding the page and an empty features.d/.
fresh() {
    rm -rf "$R" && mkdir -p "$R/docs" "$R/features.d"
    printf '%s\n' "$page" >"$P"
    printf '# features.d\n' >"$R/features.d/README.md"
}
commit() { g init -q -b main && g add -A && g commit -q -m base; }
frag() { printf '%b' "$2" >"$R/features.d/$1"; }
gather() { FS_CORE_CALLER="$R" bash "$SCRIPT" "$@" 2>&1; }

# --- Add, replace, remove and rename, in file-name order. ----------------
fresh
# Written b before a, so only a sort by name puts a's row first.
frag feat-b.md '## Reading\n\n| Inodes | Supported | 0.1.0; v3 cores Unreleased (#12) | | `b.rs`, `v3.rs` |\n\n## Writing\n\n| Append | Experimental | Unreleased (#13) | #20 | `app.rs` |\n'
frag feat-a.md '## Writing\n\n| Truncate | Experimental | Unreleased (#11) | | `t.rs` |\n\n## Reading\n\nrename: Old name\n| New name | Supported | Unreleased (#11) | | `c.rs` |\n'
frag fix-c.md '## Writing\n\nremove: Dropped thing\n'
commit
out="$(gather 0.4.0)"
status=$?
want='# Features

Work merged after the last release is **Unreleased (#N)** until the next one.

## Reading

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Superblock | Supported | 0.1.0 | | `a.rs` |
| Inodes | Supported | 0.1.0; v3 cores 0.4.0 (#12) | | `b.rs`, `v3.rs` |
| New name | Supported | 0.4.0 (#11) | | `c.rs` |

## Writing

Prose before the table.

| Feature | State | Since | Tracking | Checked by |
|---|---|---|---|---|
| Overwrite in place | Supported | 0.3.0 | | `w.rs` |
| Truncate | Experimental | 0.4.0 (#11) | | `t.rs` |
| Append | Experimental | 0.4.0 (#13) | #20 | `app.rs` |

The end.'
got="$(cat "$P")"
if [ "$status" -eq 0 ] && [ "$got" = "$want" ]; then
    ok "three fragments applied: the page is exactly what they describe"
else
    fail "three fragments: status $status ($out); docs/features.md is:
$got"
fi
row() { grep -c -F -x -- "$1" "$P"; }
# check DESCRIPTION CONDITION...: ok when the condition holds, fail naming it.
check() {
    local what="$1"; shift
    if "$@"; then ok "$what"; else fail "$what"; fi
}
replaced() {
    [ "$(row '| Inodes | Supported | 0.1.0; v3 cores 0.4.0 (#12) | | `b.rs`, `v3.rs` |')" -eq 1 ] &&
        [ "$(grep -n '^| Inodes ' "$P" | cut -d: -f1)" = 10 ]
}
check "replace: a row whose key the section holds is replaced where it stands" replaced
added() { [ "$(row '| Append | Experimental | 0.4.0 (#13) | #20 | `app.rs` |')" -eq 1 ]; }
check "add: a new key is appended to its section's table" added
removed() { [ "$(grep -c '^| Dropped thing ' "$P")" -eq 0 ] && [ "$(row '| Overwrite in place | Supported | 0.3.0 | | `w.rs` |')" -eq 1 ]; }
check "remove: the row with exactly that key is gone, and its neighbour stays" removed
renamed() {
    [ "$(grep -c '^| Old name ' "$P")" -eq 0 ] &&
        [ "$(sed -n '11p' "$P")" = '| New name | Supported | 0.4.0 (#11) | | `c.rs` |' ]
}
check "rename: the new key takes the old row's place, and the old key is gone" renamed
ordered() {
    local a b
    a="$(grep -n '^| Truncate ' "$P" | cut -d: -f1)"
    b="$(grep -n '^| Append ' "$P" | cut -d: -f1)"
    [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ]
}
check "order: fragments apply in file-name order, so feat-a's row comes before feat-b's" ordered
shipped() { [ "$(grep '^|' "$P" | grep -c 'Unreleased')" -eq 0 ] && grep -q -F 'is **Unreleased (#N)** until' "$P"; }
check "Unreleased becomes the version in every Since cell, and prose is left alone" shipped
gone() { [ "$(cd "$R/features.d" && ls -A)" = README.md ]; }
check "only README.md is left in features.d/" gone
staged() { [ "$(git -C "$R" diff --cached --name-status | sort | tr '\n' ' ')" = "D	features.d/feat-a.md D	features.d/feat-b.md D	features.d/fix-c.md " ]; }
check "the fragments' deletion is staged for the release commit" staged

# --- Unreleased the page still holds becomes the version too. ------------
fresh
sed -i.bak 's/^| Old name | Partial | 0.2.0 | #7 |/| Old name | Partial | 0.2.0; more Unreleased (#5) | #7 |/' "$P" && rm -f "$P.bak"
commit
out="$(gather v0.4.0)"
status=$?
if [ "$status" -eq 0 ] && grep -q -F -x '| Old name | Partial | 0.2.0; more 0.4.0 (#5) | #7 | `c.rs` |' "$P"; then
    ok "with no fragments, an Unreleased cell left on the page becomes the version, and a leading v is dropped"
else
    fail "a leftover Unreleased cell: status $status ($out): $(grep -n 'Old name' "$P")"
fi

# --- A repository with no features page and no fragments. ----------------
rm -rf "$R" && mkdir -p "$R"
commit
out="$(gather 0.4.0)"
status=$?
if [ "$status" -eq 0 ] && [ ! -e "$P" ]; then
    ok "no docs/features.md and no fragments: nothing to gather, and nothing is written"
else
    fail "no page: status $status ($out)"
fi

# --- Refusals change nothing. --------------------------------------------
snapshot() { (cd "$R" && cat docs/features.md 2>/dev/null; ls -A features.d 2>/dev/null; cat features.d/*.md 2>/dev/null); }
refused() {
    local what="$1" must="$2"; shift 2
    local before after out status
    before="$(snapshot)"
    out="$(gather "$@")"
    status=$?
    after="$(snapshot)"
    if [ "$status" -ne 0 ] && [ "$before" = "$after" ] && printf '%s' "$out" | grep -q -F -- "$must"; then
        ok "$what: refused, nothing changed"
    else
        fail "$what: status $status, changed: $([ "$before" = "$after" ] && echo no || echo yes), wanted '$must' in: $out"
    fi
}

fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
frag feat-b.md '## Reading\n\n| Inodes | Partial | Unreleased (#2) | | `b.rs` |\n'
commit
refused "two fragments writing the same key of the same section" "features.d/feat-a.md:3 and features.d/feat-b.md:3 both write \`Inodes\`" 0.4.0

fresh
frag feat-a.md '## Reading\n\nrename: Old name\n| Renamed | Partial | Unreleased (#1) | #7 | `c.rs` |\n'
frag feat-b.md '## Reading\n\n| Old name | Supported | Unreleased (#2) | #7 | `c.rs` |\n'
commit
refused "a row another fragment renames away" "both write \`Old name\`" 0.4.0

fresh
frag feat-a.md '## Writing\n\n| Truncate | Experimental | Unreleased (#11) | | `t.rs` |\n'
frag feat-z.md '## Mounting\n\n| Remount | Experimental | Unreleased (#12) | | `m.rs` |\n'
commit
refused "a section the page does not have, beside a fragment that would apply (no half-write)" "has no section \`## Mounting\`" 0.4.0

fresh
frag fix-a.md '## Writing\n\nremove: Never there\n'
commit
refused "removing a key the section does not hold" "there is no row \`Never there\` in \`## Writing\` to remove" 0.4.0

fresh
frag fix-a.md '## Reading\n\nrename: Never there\n| Something | Supported | Unreleased (#1) | | `s.rs` |\n'
commit
refused "renaming a key the section does not hold" "there is no row \`Never there\` in \`## Reading\` to rename" 0.4.0

fresh
frag fix-a.md '## Reading\n\nrename: Old name\n| Inodes | Supported | Unreleased (#1) | | `c.rs` |\n'
commit
refused "renaming onto a key the section already holds" "already has a row \`Inodes\`" 0.4.0

fresh
frag feat-a.md '## Reading\n\n| Short | Supported | Unreleased (#1) |\n'
commit
refused "a row with fewer cells than the table has columns" "the row has 3 cell(s), and \`## Reading\` has 5 columns" 0.4.0

fresh
frag feat-a.md 'A sentence first.\n\n## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
commit
refused "a malformed fragment" "text before the first" 0.4.0

rm -rf "$R" && mkdir -p "$R/features.d"
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
commit
refused "fragments with no docs/features.md to apply them to" "there is no docs/features.md" 0.4.0

fresh
frag feat-a.md '## Reading\n\n| Inodes | Supported | Unreleased (#1) | | `b.rs` |\n'
commit
refused "no version" "usage"
refused "a version that is not one" "is not a version" '0.4.0 | x'

if [ "$fails" -eq 0 ]; then
    echo "$NAME: all checks passed"
else
    echo "$NAME: $fails check(s) failed" >&2
fi
exit $(( fails > 0 ))
