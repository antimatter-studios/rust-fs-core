#!/usr/bin/env bash
# Print the branch this branch's pull request should be based on: `main`, or
# the head branch of the newest older open pull request it overlaps.
#
# Two open pull requests that change the same lines conflict: whichever
# merges second has to be rebased and run again, and two agents each fixing
# up the other's conflict waste a run apiece. So a pull request that would
# conflict with an older open one is based on that one's branch instead --
# stacked -- and merges after it. The shared agent-core block (v6) says when;
# this script answers it, the same way for every agent, so two agents asking
# about the same pair of branches never choose differently.
#
# OVERLAP MEANS A CONFLICT, NOT A SHARED FILE. Most pull requests in a
# repository touch the same few files -- docs/features.md, a CHANGELOG -- in
# different places, and merge cleanly in any order. Stacking on every shared
# file would serialise all of them. A trial merge (`git merge-tree`, which
# touches neither the index nor the work tree) says whether two branches
# really collide.
#
# An open pull request is a candidate when this branch would conflict with
# it, or when this branch already contains it (it was stacked there before,
# and leaving would orphan it). Of the candidates, only pull requests older
# than this one count -- a lower number -- so the newer of a pair stacks on
# the older and never the reverse; with no pull request yet, every open one is
# older. The newest candidate wins: an older overlapping pull request is
# already beneath it, by the same rule.
#
# When the base merges, GitHub retargets the stacked pull request at the
# base's own base. Run this again then, and rebase onto what it prints with
# `git rebase --onto <new base> <old base>`: main takes squash merges, so the
# base's commits are not in main under their own hashes.
#
# Usage: stack-base.sh
# Run from inside the branch's work tree, after committing.
#
# Inputs, each with a default that asks git or GitHub (tests set them):
#   STACK_BASE_PRS     "NUMBER BRANCH" lines, the repository's open pull
#                      requests (default: gh pr list --state open)
#   STACK_BASE_SELF    this branch's pull request number, empty when it has
#                      none yet (default: gh pr view)
#   STACK_BASE_MAIN    the main branch's name (default: main)
#   STACK_BASE_REMOTE  where pull request branches are read from, as
#                      REMOTE/BRANCH; empty means local branches (default:
#                      origin, fetched first)
set -euo pipefail

die() { echo "stack-base: $*" >&2; exit 1; }

git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"
current="$(git symbolic-ref --quiet --short HEAD)" || die "HEAD is detached: check out the pull request's branch"

main="${STACK_BASE_MAIN:-main}"
remote="${STACK_BASE_REMOTE-origin}"

if [ -n "$remote" ] && [ -z "${STACK_BASE_PRS+set}" ]; then
    git fetch --quiet "$remote" || die "git fetch $remote failed"
fi

if [ -n "${STACK_BASE_PRS+set}" ]; then
    prs="$STACK_BASE_PRS"
else
    command -v gh >/dev/null 2>&1 || die "gh is not installed: it lists the open pull requests"
    prs="$(gh pr list --state open --limit 500 --json number,headRefName \
        --jq '.[] | "\(.number) \(.headRefName)"')" || die "gh pr list failed"
fi

if [ -n "${STACK_BASE_SELF+set}" ]; then
    self="$STACK_BASE_SELF"
else
    self="$(gh pr view --json number --jq .number 2>/dev/null || true)"
fi
case "$self" in
    '' | *[!0-9]*) self="" ;;
esac

ref_of() {
    if [ -n "$remote" ]; then
        printf '%s/%s' "$remote" "$1"
    else
        printf '%s' "$1"
    fi
}

main_ref="$(ref_of "$main")"
git rev-parse --verify --quiet "$main_ref^{commit}" >/dev/null || die "cannot resolve $main_ref"

# Newest first, so the first candidate found is the answer.
sorted="$(printf '%s\n' "$prs" | awk 'NF >= 2 && $1 ~ /^[0-9]+$/ { print $1, $2 }' | sort -k1,1nr)"

while read -r number branch; do
    [ -n "${number:-}" ] || continue
    [ "$branch" = "$current" ] && continue
    [ "$branch" = "$main" ] && continue
    if [ -n "$self" ]; then
        [ "$number" = "$self" ] && continue
        [ "$number" -gt "$self" ] && continue
    fi
    ref="$(ref_of "$branch")"
    git rev-parse --verify --quiet "$ref^{commit}" >/dev/null ||
        die "pull request #$number's branch $ref cannot be resolved; fetch it and run again"
    # Already built on it: it stays beneath this branch until it merges.
    if git merge-base --is-ancestor "$ref" HEAD &&
        ! git merge-base --is-ancestor "$ref" "$main_ref"; then
        echo "$branch"
        exit 0
    fi
    # Exit 0: merges cleanly. 1: conflicts. Anything else: git failed, which
    # is not an answer.
    set +e
    git merge-tree --write-tree "$ref" HEAD >/dev/null 2>&1
    status=$?
    set -e
    case "$status" in
        0) ;;
        1) echo "$branch"; exit 0 ;;
        *) die "git merge-tree failed ($status) against pull request #$number ($ref)" ;;
    esac
done <<<"$sorted"

echo "$main"
