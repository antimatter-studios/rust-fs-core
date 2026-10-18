### Added

- **A partial result has its own exit status.** `cli::output::EXIT_PARTIAL`
  is 5: the operation completed and its output is usable, but some data
  could not be recovered, and the report says which. A rescue or salvage
  verb exits with it instead of 0, which says every byte was recovered, or
  1, which says there is nothing to use.
