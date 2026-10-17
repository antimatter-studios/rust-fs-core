### Added

- **Tree imports preserve times, hard links and symbolic links through optional sink capabilities.** `--preserve` on import accepts `times`, `links` or `times,links`. Sinks gain `set_times`, `create_hard_link` and a `format_name`; repeated names of one source file become hard links to the first published name, and directory times are applied after their children. A sink lacking a capability reports each affected item as `refused: <capability> not supported by <format>` and the run exits nonzero, instead of dropping the metadata.
