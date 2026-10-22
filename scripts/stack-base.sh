#!/usr/bin/env bash
# Print the branch this branch's pull request should be based on: the head
# branch of the top of the repository's one stack, or `main` when no pull
# request is open.
#
# Every repository lands its pull requests as one stack (agent-core v7),
# whether or not they touch the same files. A new pull request goes on the
# top: its author rebases onto the top, resolves any conflict locally while
# doing so, and checks the result builds before pushing, so a conflict never
# reaches the merge queue and is paid for once, by the pull request that
# brings it. This script names the top, the same way for every agent, so two
# agents opening pull requests at once ask the same question and get the same
# answer.
#
# The stack is GitHub's own (`gh stack link`), read from each open pull
# request's `stack` field:
#
#   * A pull request already in the stack is based on the nearest open layer
#     below it, or `main` at the bottom (GitHub's positions count layers that
#     have merged). Ask again whenever a layer below merges,
#     and rebase onto the answer with `git rebase --onto <new base> <old
#     base>`: main takes squash merges, so the old base's commits are not in
#     main under their own hashes.
#   * A pull request not yet in it goes on the top: the layer with the
#     highest position.
#   * With no stack yet, a new pull request stacks on the newest older open
#     pull request, and so starts the stack; with none open, on `main`.
#   * Multiple native stack segments are accepted only when their base/head
#     branches prove one complete chain from main, in each segment's native
#     order. Positions restart in each segment, so they cannot order the
#     segments themselves. Missing links, forks and cycles are refused.
#
# A pull request taken out of the stack because its fix cannot come soon is
# open but in no stack; it is never anyone's base.
#
# Usage: stack-base.sh
# Run from inside the branch's work tree.
#
# Inputs, each with a default that asks git or GitHub (tests set them):
#   STACK_BASE_PRS   "NUMBER BRANCH STACK POSITION BASE" lines, the repository's
#                    open pull requests, with "-" for STACK and POSITION
#                    when a pull request is in no stack (default: the
#                    GitHub API, through gh). Legacy four-column inputs
#                    still work for a single stack; joining segments needs BASE.
#   STACK_BASE_SELF  this branch's pull request number, empty when it has
#                    none yet (default: the open pull request whose head is
#                    this branch)
#   STACK_BASE_MAIN  the main branch's name (default: main)
set -euo pipefail

die() { echo "stack-base: $*" >&2; exit 1; }

git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"
current="$(git symbolic-ref --quiet --short HEAD)" || die "HEAD is detached: check out the pull request's branch"

main="${STACK_BASE_MAIN:-main}"

if [ -n "${STACK_BASE_PRS+set}" ]; then
    prs="$STACK_BASE_PRS"
else
    command -v gh >/dev/null 2>&1 || die "gh is not installed: it lists the open pull requests"
    prs="$(gh api --paginate 'repos/{owner}/{repo}/pulls?state=open&per_page=100' \
        --jq '.[] | "\(.number) \(.head.ref) \(.stack.number // "-") \(.stack.position // "-") \(.base.ref)"')" ||
        die "listing the open pull requests failed"
fi

# Well-formed lines only: a number, a branch, and a stack and position that
# are both numbers or both "-".
rows="$(printf '%s\n' "$prs" | awk '
    NF >= 4 && $1 ~ /^[0-9]+$/ &&
    (($3 ~ /^[0-9]+$/ && $4 ~ /^[0-9]+$/) || ($3 == "-" && $4 == "-")) {
        print $1, $2, $3, $4, ($5 == "" ? "-" : $5)
    }')"

if [ -n "${STACK_BASE_SELF+set}" ]; then
    self="$STACK_BASE_SELF"
else
    self="$(printf '%s\n' "$rows" | awk -v b="$current" '$2 == b { print $1; exit }')"
fi
case "$self" in
    '' | *[!0-9]*) self="" ;;
esac

stacks="$(printf '%s\n' "$rows" | awk '$3 != "-" { print $3 }' | sort -un)"
if [ "$(printf '%s\n' "$stacks" | grep -c .)" -gt 1 ]; then
    # A native stack ID is a segment, not proof that its branch chain is
    # disconnected. Validate every link before selecting any branch.
    if chain_base="$(printf '%s\n' "$rows" | awk -v self="$self" -v main="$main" '
        function refuse(message) { print message; exit 1 }
        $3 != "-" {
            n++
            number[n] = $1
            head[n] = $2
            stack[n] = $3
            position[n] = $4
            base[n] = $5
            if ($2 in by_head) problem = "duplicate head branches"
            if ($1 in by_number) problem = "duplicate pull requests"
            by_head[$2] = n
            by_number[$1] = n
            if ($5 == "-") problem = "missing base branch metadata"
        }
        END {
            if (problem != "") refuse(problem)
            for (i = 1; i <= n; i++) {
                if (base[i] in by_head) {
                    parent = by_head[base[i]]
                    if (child[parent]) refuse("a branch has multiple successors")
                    child[parent] = i
                } else if (base[i] == main) {
                    if (root) refuse("disconnected roots")
                    root = i
                } else {
                    refuse("a predecessor is missing")
                }
            }
            if (!root) refuse("no chain starts at the main branch")
            current = root
            while (current) {
                if (visited[current]) refuse("a branch cycle")
                visited[current] = 1
                count++
                group = stack[current]
                if (group != previous_group) {
                    if (seen_group[group]) refuse("interleaved native segments")
                    seen_group[group] = 1
                } else if (position[current] + 0 <= previous_position + 0) {
                    refuse("native positions contradict branch order")
                }
                if (number[current] == self) {
                    own_base = previous ? head[previous] : main
                    own_base_found = 1
                }
                previous = current
                previous_group = group
                previous_position = position[current]
                current = child[current]
            }
            if (count != n) refuse("disconnected or cyclic branches")
            print own_base_found ? own_base : head[previous]
        }
    ')"; then
        printf '%s\n' "$chain_base"
        exit 0
    fi
    detail="$(printf '%s\n' "$rows" | awk '$3 != "-" { printf "  stack %s: #%s (%s, position %s)\n", $3, $1, $2, $4 }' | sort)"
    die "the open pull requests are in more than one stack without one verified branch chain ($chain_base):
$detail"
fi

if [ -n "$stacks" ]; then
    # This pull request's own layer: based on the one just below it.
    if [ -n "$self" ]; then
        mine="$(printf '%s\n' "$rows" | awk -v s="$self" '$1 == s && $3 != "-" { print $4 }')"
        if [ -n "$mine" ]; then
            # Positions count layers that have merged, so the layer below is
            # the nearest open one beneath, and `main` when none is left.
            below="$(printf '%s\n' "$rows" | awk -v p="$mine" '$3 != "-" && $4 + 0 < p + 0 { print $4, $2 }' |
                sort -k1,1nr | awk 'NR == 1 { print $2 }')"
            echo "${below:-$main}"
            exit 0
        fi
    fi
    # Not in it yet: the top, the highest position.
    printf '%s\n' "$rows" | awk -v s="$self" '$3 != "-" && $1 != s { print $4, $2 }' |
        sort -k1,1nr | awk 'NR == 1 { print $2 }'
    exit 0
fi

# No stack yet: the newest older open pull request starts it.
top="$(printf '%s\n' "$rows" | awk -v s="$self" -v b="$current" -v m="$main" '
    $2 != b && $2 != m && (s == "" || ($1 != s && $1 + 0 < s + 0)) { print $1, $2 }' |
    sort -k1,1nr | awk 'NR == 1 { print $2 }')"
echo "${top:-$main}"
