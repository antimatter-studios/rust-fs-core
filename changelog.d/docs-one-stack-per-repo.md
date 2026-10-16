### Changed

- **Every repository lands its pull requests as one stack (agent-core v7).**
  A new pull request goes on the top of the stack, whether or not it touches
  the same files as the layers below, with its conflicts resolved locally
  before it is pushed; each layer is one squashed commit; the stack is queued
  with `gh stack merge`; and a red layer is fixed in place, with the layers
  above replayed by `git rebase --update-refs`. The block's digest changes,
  so each repository moves to this release's pin together with its
  `AGENTS.md`.
- **`scripts/stack-base.sh` prints the top of the repository's stack.** It
  reads GitHub's stack from the open pull requests: a pull request not yet in
  the stack is based on its top, one already in it on the nearest open layer
  below, and `main` when nothing is open. Open pull requests in more than one
  stack are refused, naming them. It no longer decides by trial merge.
