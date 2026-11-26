### Fixed

- **A recursive export or import on Windows names files inside the image with
  `/`.** The shared traversal joined image paths with the host's separator, so a
  driver found no nested entry, and the reports held backslashes (#489).
