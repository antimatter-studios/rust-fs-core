# Selecting a stack base

Run `scripts/stack-base.sh` in the repository whose pull requests are being
stacked. It reads the open pull requests from GitHub and prints one branch:
the top for new work, the nearest open predecessor for an existing layer,
or `main` when nothing is open. `STACK_BASE_MAIN` selects a different trunk.

One dependency chain may span more than one native GitHub stack segment.
Native positions restart in each segment, so neither the largest position
nor the largest stack or pull request number identifies the chain's top.
The helper reads each pull request's base branch and verifies the links
before accepting multiple segments. This follows GitHub's stack contract:
each pull request's base matches the preceding pull request's head.

For example, these rows form one chain despite their two stack IDs:

```text
PR  head                 stack  position  base
20  feat/first-bottom    7      98        main
21  feat/first-top       7      100       feat/first-bottom
51  feat/second-bottom   9      1         feat/first-top
52  feat/second-top      9      2         feat/second-bottom
```

New work is based on `feat/second-top`. PR 51 is based on `feat/first-top`,
across the segment boundary. Gaps left by merged layers are allowed.

Accepting multiple segments requires one root at the configured trunk,
unique head branches, one successor per branch, all participating layers
reachable without cycles, and increasing native positions within each
contiguous segment. Missing base metadata, missing predecessors, forks,
cycles, disconnected roots, duplicate heads or contradictory ordering fail
explicitly. An unstacked pull request is never selected as a base.

The existing single-stack position ordering and no-stack fallback remain
available. Test fixtures using the legacy four-column `STACK_BASE_PRS`
input still work for those cases; proving a connection across multiple
native stacks requires the fifth base-branch column.

`tests/scripts/test-stack-base.sh` covers these cases and the production
GitHub API query. It runs with the existing shell suite in GitHub Actions.

Reference: [GitHub's ordered stack API contract](https://docs.github.com/en/rest/pulls/stacks#create-a-pull-request-stack).
