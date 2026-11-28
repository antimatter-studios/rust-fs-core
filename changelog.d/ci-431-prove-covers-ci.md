### Changed

- **`scripts/prove.sh` names a log for every task.** Each task's output is kept in `tmp/logs/prove-TASK.log`, a tier log is named only when the run wrote it, `--not-covered JOB` lists the CI jobs an arm64 Mac does not cover, and `--version` states the contract.
