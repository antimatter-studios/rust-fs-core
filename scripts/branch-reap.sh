#!/usr/bin/env bash
#
# branch-reap.sh REPO BRANCH MERGED_SHA [PR] -- delete a merged pull
# request's branch, unless something still needs it.
#
# Run by .github/workflows/branch-reap.yml when a pull request merges, with
# the pull request's head branch and the commit that merged. Deletes the
# branch only when ALL of these hold:
#   - it is not the repository's default branch, not protected, not a
#     merge-queue branch (gh-readonly-queue/) and not a wip/ snapshot;
#   - no open pull request has it as its base -- a stacked pull request
#     still built on it -- or as its head;
#   - it still points at MERGED_SHA. That last check IS the delete: a push
#     with --force-with-lease=<branch>:<sha> succeeds only if the server
#     still has the branch at that commit, so a commit pushed while this
#     ran is never deleted with it.
# Anything else is kept, and says why. Keeping is never a failure: the
# branch is left for a person or for a later sweep (diskjockey's
# am-branch-reap), which applies the same rules.
#
# WHY NOT GITHUB'S "AUTOMATICALLY DELETE HEAD BRANCHES". It deletes the
# branch the moment its pull request merges, whatever is still based on it,
# and the family has already lost a stacked pull request to that race.
#
#   BRANCH_REAP_GH      the gh command (tests replace it)
#   BRANCH_REAP_REMOTE  where the branch lives (default
#                       https://github.com/REPO.git; tests use a bare repo)
set -euo pipefail

repo="${1:?usage: branch-reap.sh REPO BRANCH MERGED_SHA [PR]}"
branch="${2:?usage: branch-reap.sh REPO BRANCH MERGED_SHA [PR]}"
sha="${3:?usage: branch-reap.sh REPO BRANCH MERGED_SHA [PR]}"
pr="${4:-}"
GH="${BRANCH_REAP_GH:-gh}"
remote="${BRANCH_REAP_REMOTE:-https://github.com/$repo.git}"
what="$branch${pr:+ (#$pr)}"

keep() { echo "branch-reap: kept $what: $1"; exit 0; }

case "$branch" in
    gh-readonly-queue/*) keep "a merge-queue branch is the queue's own" ;;
    wip/*) keep "a wip/ snapshot is kept for a person" ;;
esac

default="$("$GH" api "repos/$repo" --jq .default_branch)"
[ "$branch" != "$default" ] || keep "it is the default branch"
protected="$("$GH" api "repos/$repo/branches/$branch" --jq .protected 2>/dev/null || echo missing)"
case "$protected" in
    missing) keep "it is already gone" ;;
    true) keep "it is protected" ;;
esac

based="$("$GH" pr list -R "$repo" --state open --base "$branch" --json number --jq '[.[].number | "#\(.)"] | join(" ")')"
[ -z "$based" ] || keep "open pull requests are based on it: $based"
headed="$("$GH" pr list -R "$repo" --state open --head "$branch" --json number --jq '[.[].number | "#\(.)"] | join(" ")')"
[ -z "$headed" ] || keep "it is the head of open pull requests: $headed"

# The push needs a repository to push from, not a checkout of this one.
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
git init -q "$scratch"
if git -C "$scratch" push --quiet --force-with-lease="refs/heads/$branch:$sha" "$remote" ":refs/heads/$branch" 2>"$scratch/err"; then
    echo "branch-reap: deleted $what at $sha"
    exit 0
fi
if grep -q "stale info" "$scratch/err"; then
    keep "it moved after the merge; it no longer points at $sha"
fi
echo "branch-reap: could not delete $what:" >&2
cat "$scratch/err" >&2
exit 1
