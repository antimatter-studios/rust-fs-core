### Added

- **The salvage conformance set runs over sources, holding no image in
  memory.** A driver implements `layers::conformance::salvage::SourceDriver` and
  calls `run_sources`: it gives a healthy source to read, describes each damaged
  case as a source of its own (`DamagedSource`, usually an `Overlay` of the
  damage on the healthy one), and its strict API is handed an owned source it can
  mount. `Driver` and `run` are unchanged (#493).
