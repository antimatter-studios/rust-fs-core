### Added

- `cli::output::CliError::not_found` reports a path that does not exist: exit 1, and `"kind": "not_found"` beside `error` and `code` in the JSON error, which a script matches across drivers whatever each one's message says (antimatter-studios/diskjockey#524). `CliError::kind()` reads it; every other error has none.

### Changed

- **Breaking:** `CliError` has a private field, so it is built with its constructors (`failed`, `usage`, `not_implemented`, `refused`, `refused_by_state`, `not_found`), not as a struct literal. `message` and `code` stay public. The crate is 0.4.0.
