### Changed

- **agent-core v8: a red run stays on its own pull request.** A pull request
  opens with only its failing test and a title that starts `red:`, and its
  own CI run of that commit is the red evidence. The fix goes onto the same
  branch, squashed to one commit, and `red:` comes off the title. No `red/*`
  branch is pushed and nothing is dispatched. `agents-core-check` expects v8.
