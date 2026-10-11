#!/usr/bin/env bash
# stack-base.sh prints the branch a pull request is based on: `main`, or the
# newest older open pull request it would conflict with or is already built
# on. Every agent asks it before opening a pull request, so the answer has to
# be the same for the same branches whoever asks, and has to be about
# conflicts, not files: two pull requests editing different parts of one file
# merge cleanly and must not be serialised.
#
#   bash tests/scripts/test-stack-base.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/stack-base.sh"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$SCRIPT" ] || { echo "FAIL  scripts/stack-base.sh is missing" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/stack-base-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

g() { git -c user.name=t -c user.email=t@example.com "$@" >/dev/null 2>&1; }
g init -q -b main .
seq 1 40 >a.txt
echo b >b.txt
g add . && g commit -q -m base

# branch NAME FROM SED-EXPR FILE: a branch off FROM that edits FILE.
branch() {
    g checkout -q -b "$1" "$2"
    sed -i.bak "$3" "$4" && rm -f "$4.bak"
    g commit -q -am "$1"
    g checkout -q main
}
branch pr-10 main 's/^2$/two (pr-10)/' a.txt     # line 2 of a.txt
branch pr-11 main 's/^b$/bee (pr-11)/' b.txt      # b.txt only
branch pr-12 main 's/^38$/thirty-eight/' a.txt    # a.txt, far from line 2
branch pr-13 main 's/^2$/TWO (pr-13)/' a.txt      # line 2 again
branch mine main 's/^2$/2 (mine)/' a.txt          # line 2: conflicts with 10 and 13
g checkout -q mine

# expect WANT DESCRIPTION SELF PRS: run on the current branch, WANT on stdout.
expect() {
    local want="$1" what="$2" self="$3" prs="$4" got
    got="$(STACK_BASE_REMOTE='' STACK_BASE_MAIN=main STACK_BASE_SELF="$self" \
        STACK_BASE_PRS="$prs" bash "$SCRIPT" 2>&1)"
    if [ "$got" = "$want" ]; then
        ok "$what: $want"
    else
        fail "$what: wanted '$want', got '$got'"
    fi
}

expect main "no open pull requests" "" ""
expect main "a pull request that edits another file" "" "11 pr-11"
expect main "a pull request that edits the same file elsewhere" "" "12 pr-12"
expect pr-10 "a pull request it conflicts with" "" "11 pr-11
10 pr-10"
expect pr-13 "the newest of two it conflicts with" "" "10 pr-10
13 pr-13
12 pr-12"
expect pr-13 "the same answer whatever order they are listed in" "" "13 pr-13
12 pr-12
10 pr-10"
expect pr-10 "only older pull requests count once it has a number" "12" "10 pr-10
13 pr-13"
expect main "a newer pull request is never a base" "9" "10 pr-10
13 pr-13"
expect main "its own pull request is not its base" "10" "10 mine"

# Already stacked: built on pr-10, with a change of its own elsewhere.
g checkout -q -b stacked pr-10
echo c >c.txt && g add c.txt && g commit -q -m stacked
expect pr-10 "a branch already built on a pull request stays on it" "" "10 pr-10
11 pr-11"
g checkout -q mine

got="$(STACK_BASE_REMOTE='' STACK_BASE_MAIN=main STACK_BASE_SELF='' \
    STACK_BASE_PRS="14 no-such-branch" bash "$SCRIPT" 2>&1)"
status=$?
if [ "$status" -ne 0 ] && [ "$got" != main ]; then
    ok "a pull request whose branch cannot be read is an error, not main"
else
    fail "an unreadable branch: wanted a failure, got status $status and '$got'"
fi

g checkout -q --detach mine
got="$(STACK_BASE_REMOTE='' STACK_BASE_SELF='' STACK_BASE_PRS='' bash "$SCRIPT" 2>&1)"
status=$?
if [ "$status" -ne 0 ]; then
    ok "a detached HEAD is refused"
else
    fail "a detached HEAD: wanted a failure, got '$got'"
fi

if [ "$fails" -gt 0 ]; then
    echo "test-stack-base: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  stack-base stacks on the newest older pull request it conflicts with, and on nothing else"
