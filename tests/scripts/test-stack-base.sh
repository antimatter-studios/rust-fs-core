#!/usr/bin/env bash
# stack-base.sh prints the branch a pull request is based on: the top of the
# repository's one stack, the nearest open layer below a pull request already
# in it, or `main`. Every agent asks it before opening a pull request, so the
# answer has to be the same whoever asks, and a repository holding two stacks
# has to be refused rather than guessed at.
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
echo a >a.txt
g add . && g commit -q -m base
g checkout -q -b mine

# expect WANT DESCRIPTION SELF PRS: run on branch `mine`, WANT on stdout.
# SELF "unset" leaves the script to find this branch's pull request itself.
expect() {
    local want="$1" what="$2" self="$3" prs="$4" got
    if [ "$self" = unset ]; then
        got="$(STACK_BASE_MAIN=main STACK_BASE_PRS="$prs" bash "$SCRIPT" 2>&1)"
    else
        got="$(STACK_BASE_MAIN=main STACK_BASE_SELF="$self" STACK_BASE_PRS="$prs" bash "$SCRIPT" 2>&1)"
    fi
    if [ "$got" = "$want" ]; then
        ok "$what: $want"
    else
        fail "$what: wanted '$want', got '$got'"
    fi
}

STACK="20 feat/bottom 7 1
23 feat/middle 7 2
21 feat/top 7 3"

expect main "no open pull requests" "" ""
expect feat/top "a new pull request goes on the top of the stack" "" "$STACK"
expect feat/top "whatever order the pull requests are listed in" "" "21 feat/top 7 3
20 feat/bottom 7 1
23 feat/middle 7 2"
expect feat/top "whether or not the files overlap: the top is the top" "30" "$STACK"
expect feat/middle "a layer is based on the layer below it" "21" "$STACK"
expect main "the bottom layer is based on main" "20" "$STACK"
expect feat/bottom "a layer above a merged one is based on the nearest open layer" "23" "20 feat/bottom 7 1
23 feat/middle 7 3"
expect main "a layer whose lower layers all merged is based on main" "23" "23 feat/middle 7 3"
expect feat/middle "the script finds this branch's own pull request" unset "20 feat/bottom 7 1
23 feat/middle 7 2
22 mine 7 3"
expect feat/top "a pull request taken out of the stack is no one's base" "" "$STACK
25 feat/red - -"
expect feat/newest "with no stack, the newest older open pull request starts it" "30" "12 feat/old - -
14 feat/newest - -"
expect feat/old "with no stack, a newer pull request is never a base" "13" "12 feat/old - -
14 feat/newest - -"
expect main "with no stack, its own pull request is not its base" "12" "12 mine - -"
expect feat/top "a malformed line is ignored" "" "$STACK
x feat/garbage 7 9
26 feat/half 7 -"

got="$(STACK_BASE_MAIN=main STACK_BASE_SELF='' STACK_BASE_PRS="$STACK
40 feat/other 9 1" bash "$SCRIPT" 2>&1)"
status=$?
if [ "$status" -ne 0 ] && printf '%s' "$got" | grep -q 'more than one stack' &&
    printf '%s' "$got" | grep -q '#40'; then
    ok "two stacks are refused, naming their pull requests"
else
    fail "two stacks: wanted a refusal naming #40, got status $status and '$got'"
fi

g checkout -q --detach mine
got="$(STACK_BASE_SELF='' STACK_BASE_PRS='' bash "$SCRIPT" 2>&1)"
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
echo "PASS  stack-base puts a new pull request on the top of the one stack, and a layer on the one below it"
