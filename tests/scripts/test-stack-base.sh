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

# Native positions restart in the second segment. Branch dependencies,
# rather than PR numbers, stack IDs or the largest native position, order it.
SEGMENTS="52 feat/second-top 9 2 feat/second-bottom
21 feat/first-top 7 100 feat/first-bottom
51 feat/second-bottom 9 1 feat/first-top
20 feat/first-bottom 7 98 main"

expect feat/second-top "connected native segments select their actual top" "" "$SEGMENTS"
expect feat/first-top "the first layer of a segment uses the preceding segment" "51" "$SEGMENTS"
expect feat/second-bottom "a layer inside the upper segment uses its predecessor" "52" "$SEGMENTS"
expect feat/first-bottom "an earlier segment still resolves its own predecessor" "21" "$SEGMENTS"
expect main "the bottom of connected segments still uses main" "20" "$SEGMENTS"
expect feat/second-top "an unstacked PR never becomes a segmented stack base" "" "$SEGMENTS
80 feat/parked - - main"
expect feat/second-top "segment ordering is independent of API order" "" "20 feat/first-bottom 7 98 main
51 feat/second-bottom 9 1 feat/first-top
21 feat/first-top 7 100 feat/first-bottom
52 feat/second-top 9 2 feat/second-bottom"
expect feat/second-top "branch detection finds an existing layer in the upper segment" unset "$SEGMENTS
53 mine 9 3 feat/second-top"

# Rejection still has to be explicit: no stdout branch may be guessed for
# disconnected, incomplete, branching, cyclic or contradictory metadata.
reject_segments() {
    local what="$1" prs="$2" got status
    got="$(STACK_BASE_MAIN=main STACK_BASE_SELF='' STACK_BASE_PRS="$prs" bash "$SCRIPT" 2>&1)"
    status=$?
    if [ "$status" -ne 0 ] && printf '%s' "$got" | grep -q 'more than one stack'; then
        ok "$what"
    else
        fail "$what: wanted an explicit refusal, got status $status and '$got'"
    fi
}

reject_segments "disconnected native segments are refused" "20 feat/a 7 1 main
51 feat/b 9 1 main"
reject_segments "a missing cross-segment predecessor is refused" "20 feat/a 7 1 main
51 feat/b 9 1 feat/missing"
reject_segments "a fork between segments is refused" "20 feat/a 7 1 main
51 feat/b 9 1 feat/a
52 feat/c 9 2 feat/a"
reject_segments "a disconnected cycle is refused" "20 feat/a 7 1 main
51 feat/b 9 1 feat/c
52 feat/c 9 2 feat/b"
reject_segments "a cycle without a root is refused" "20 feat/a 7 1 feat/b
51 feat/b 9 1 feat/a"
reject_segments "duplicate head branches are refused" "20 feat/a 7 1 main
51 feat/a 9 1 feat/a"
reject_segments "native order contradicting branch dependencies is refused" "20 feat/a 7 1 main
51 feat/b 9 2 feat/a
52 feat/c 9 1 feat/b"
reject_segments "interleaved native segments are refused" "20 feat/a 7 1 main
51 feat/b 9 1 feat/a
21 feat/c 7 2 feat/b
52 feat/d 9 2 feat/c"
reject_segments "missing base metadata does not authorize joining segments" "20 feat/a 7 1
51 feat/b 9 1"

# Exercise the production gh path, not only the injected row contract. A
# successful query must ask GitHub for .base.ref as linking evidence.
mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'GH'
#!/usr/bin/env bash
case "$*" in
    *'.base.ref'*) printf '%s\n' "$STACK_TEST_PRS" ;;
    *) echo 'base branch was not requested from GitHub' >&2; exit 91 ;;
esac
GH
chmod +x "$work/bin/gh"
got="$(PATH="$work/bin:$PATH" STACK_TEST_PRS="$SEGMENTS" STACK_BASE_MAIN=main STACK_BASE_SELF='' bash "$SCRIPT" 2>&1)"
status=$?
if [ "$status" -eq 0 ] && [ "$got" = feat/second-top ]; then
    ok "the live API adapter requests base branches and selects the connected top"
else
    fail "the live API adapter: wanted feat/second-top with status 0, got status $status and '$got'"
fi


# A fork's branch exists only on the fork, so no pull request here can be based
# on it and `git rebase` has nothing to resolve it to. A row names its head and
# base repositories; one that differs is left out before any rule is applied.
expect main "with no stack, a fork's pull request is no one's base" "" "605 feat/fork - - main someone/repo acme/repo"
expect main "with no stack, a pull request older than this one from a fork is no one's base" "610" "605 feat/fork - - main someone/repo acme/repo"
expect feat/ours "with no stack, an older same-repository pull request beats a newer fork's" "" "600 feat/ours - - main acme/repo acme/repo
605 feat/fork - - main someone/repo acme/repo"
expect feat/ours "the same, asked by an existing pull request above both" "610" "600 feat/ours - - main acme/repo acme/repo
605 feat/fork - - main someone/repo acme/repo"
expect main "a head repository that was deleted is not this repository" "" "605 feat/gone - - main - acme/repo"
expect main "a head repository with no base repository to compare is ignored" "" "605 feat/half - - main acme/repo"
expect feat/bottom "a fork's pull request is never the top of a stack" "" "20 feat/bottom 7 1 main acme/repo acme/repo
21 feat/fork 7 2 feat/bottom someone/repo acme/repo"
expect feat/bottom "a fork's pull request in another stack does not make two stacks" "" "20 feat/bottom 7 1 main acme/repo acme/repo
21 feat/fork 9 1 main someone/repo acme/repo"
expect feat/old "a fork's branch of this branch's name is not this branch's pull request" unset "12 feat/old - - main acme/repo acme/repo
9 mine - - main someone/repo acme/repo"

# The rule has to hold on what GitHub sends, so this stub applies the script's
# own --jq filter to a response of GitHub's shape instead of returning rows.
command -v jq >/dev/null 2>&1 ||
    { echo "FAIL  jq is required to run this test: install jq" >&2; exit 1; }
mkdir -p "$work/api"
cat > "$work/api/gh" <<'GH'
#!/usr/bin/env bash
while [ "$#" -gt 0 ]; do
    if [ "$1" = --jq ]; then
        exec jq -r "$2" "$STACK_TEST_RESPONSE"
    fi
    shift
done
echo 'no --jq filter was passed to gh' >&2
exit 91
GH
chmod +x "$work/api/gh"

# api_pr NUMBER BRANCH HEAD-REPO: one element of GitHub's pull request list.
# HEAD-REPO is a quoted JSON value, because GitHub sends null for a deleted fork.
api_pr() {
    printf '{"number":%s,"head":{"ref":"%s","repo":%s},"base":{"ref":"main","repo":{"full_name":"acme/repo"}}}' \
        "$1" "$2" "$3"
}
OURS='{"full_name":"acme/repo"}'
FORK='{"full_name":"someone/repo"}'

# expect_api WANT DESCRIPTION RESPONSE: the answer through the default listing.
expect_api() {
    local want="$1" what="$2" got status
    printf '%s\n' "$3" >"$work/response.json"
    got="$(PATH="$work/api:$PATH" STACK_TEST_RESPONSE="$work/response.json" STACK_BASE_MAIN=main STACK_BASE_SELF='' bash "$SCRIPT" 2>&1)"
    status=$?
    if [ "$status" -eq 0 ] && [ "$got" = "$want" ]; then
        ok "the default listing: $what: $want"
    else
        fail "the default listing: $what: wanted '$want' with status 0, got status $status and '$got'"
    fi
}

expect_api main "a fork's pull request is no one's base" "[$(api_pr 605 feat/fork "$FORK")]"
expect_api main "a deleted fork's pull request is no one's base" "[$(api_pr 605 feat/gone null)]"
expect_api feat/ours "an older same-repository pull request beats a newer fork's" \
    "[$(api_pr 605 feat/fork "$FORK"),$(api_pr 600 feat/ours "$OURS")]"

expect main "with no stack, a red draft is never a base" "" \
    "40 feat/red - - main acme/repo acme/repo true red: failing test"
expect main "with no stack, a draft without a red title is never a base" "" \
    "40 feat/draft - - main acme/repo acme/repo true Work in progress"
expect main "with no stack, a ready pull request titled red is never a base" "" \
    "40 feat/red - - main acme/repo acme/repo false red: failing test"
expect feat/ready "with no stack, a ready same-repository pull request starts it" "" \
    "40 feat/ready - - main acme/repo acme/repo false A ready change"
expect feat/ready "with no stack, the newest eligible older pull request wins" "50" \
    "41 feat/draft - - main acme/repo acme/repo true Draft
42 feat/red - - main acme/repo acme/repo false red: failing test
39 feat/ready - - main acme/repo acme/repo false Ready
38 feat/older - - main acme/repo acme/repo false Older
51 feat/newer - - main acme/repo acme/repo false Newer"
expect main "with no stack, only drafts and red pull requests leave main" "" \
    "40 feat/draft - - main acme/repo acme/repo true Draft
41 feat/red - - main acme/repo acme/repo false red: failing test"
expect feat/parked "with no stack, a ready pull request taken out of a stack is eligible again" "" \
    "40 feat/parked - - main acme/repo acme/repo false Ready again"
expect main "with no stack, a draft taken out of a stack stays ineligible" "" \
    "40 feat/parked - - main acme/repo acme/repo true Parked"
expect feat/older "a draft still identifies this branch's own pull request" unset \
    "40 feat/older - - main acme/repo acme/repo false Older
41 mine - - main acme/repo acme/repo true Draft
42 feat/newer - - main acme/repo acme/repo false Newer"
expect feat/legacy "legacy seven-column input remains eligible without readiness metadata" "" \
    "40 feat/legacy - - main acme/repo acme/repo"
expect feat/redwood "a title merely starting with red is not the red prefix" "" \
    "40 feat/redwood - - main acme/repo acme/repo false redwood fixes"
expect feat/ready "red later in a title does not mark a pull request red" "" \
    "40 feat/ready - - main acme/repo acme/repo false fix: red: text"
expect feat/top "draft and red metadata do not change an existing stack's top" "" \
    "20 feat/bottom 7 1 main acme/repo acme/repo false Ready
21 feat/top 7 2 feat/bottom acme/repo acme/repo true red: failing test"
expect feat/bottom "draft and red metadata do not change an existing layer's predecessor" "21" \
    "20 feat/bottom 7 1 main acme/repo acme/repo true red: failing test
21 feat/top 7 2 feat/bottom acme/repo acme/repo false Ready"
expect feat/second-top "draft and red metadata do not change connected native segments" "" \
    "20 feat/first-bottom 7 98 main acme/repo acme/repo false Ready
21 feat/first-top 7 100 feat/first-bottom acme/repo acme/repo true red: first
51 feat/second-bottom 9 1 feat/first-top acme/repo acme/repo false Ready
52 feat/second-top 9 2 feat/second-bottom acme/repo acme/repo true red: top"

api_pr_state() {
    api_pr "$1" "$2" "$OURS" |
        jq -c --argjson draft "$3" --arg title "$4" '. + {draft: $draft, title: $title}'
}

expect_api main "a red draft is never a base" \
    "[$(api_pr_state 40 feat/red true 'red: failing test')]"
expect_api main "a draft without a red title is never a base" \
    "[$(api_pr_state 40 feat/draft true 'Work in progress')]"
expect_api main "a ready pull request titled red is never a base" \
    "[$(api_pr_state 40 feat/red false 'red: failing test')]"
expect_api feat/ready "a ready same-repository pull request remains eligible" \
    "[$(api_pr_state 40 feat/ready false 'A ready change')]"
expect_api feat/ready "newer drafts and red titles cannot hide an eligible base" \
    "[$(api_pr_state 42 feat/draft true Draft),$(api_pr_state 41 feat/red false 'red: failing test'),$(api_pr_state 40 feat/ready false Ready)]"
expect_api feat/ready "red later in a title does not mark it red" \
    "[$(api_pr_state 40 feat/ready false 'fix: red: text')]"
expect_api main "a red title with a newline cannot introduce a candidate row" \
    "[$(api_pr_state 40 feat/red false $'red: failing test\n99 feat/injected - - main acme/repo acme/repo false Ready')]"

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
