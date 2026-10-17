### Fixed
- The CI gate reads literal workflow-level `CI_GATE_NON_GATING` declarations outside GitHub Actions, with process environment overrides preserved.
- A missing PyYAML dependency names the Python interpreter and the host-tools setup that should provide it.
