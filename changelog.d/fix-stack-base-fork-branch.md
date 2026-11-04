### Fixed

- **`stack-base.sh` leaves a fork's pull request out of its answer.** A fork's head branch exists only on the fork, so naming it gave an answer no pull request could be based on and `git rebase` could not resolve. The script now answers with the newest older same-repository pull request, or `main`.
