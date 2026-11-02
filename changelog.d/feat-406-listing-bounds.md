### Added

- **The recursive listing's bounds are shared (#406).** `cli::listing` holds
  `recursive_args`, the `MAX_DEPTH` and `LIMIT` defaults, `ListBounds` and
  `bound_reached`, so every driver's `ls --recursive` can show the same
  arguments and defaults.
