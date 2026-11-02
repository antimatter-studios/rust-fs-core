### Changed

- **A partial tree transfer exits 5 (#361).** A recursive export or import that
  ran to its end, published what it could and reported the rest as refused or
  failed now exits with `EXIT_PARTIAL` instead of 1. A run that published
  nothing still exits 1. A script that checks for exactly 1 needs updating.
