### Fixed

- **Windows release binaries start without the Visual C++ runtime (#411).**
  `release-cli.yml` links the C runtime statically on its Windows legs, and
  `package-cli.sh` refuses a Windows binary that imports `VCRUNTIME140.dll`. A
  caller that builds for `package-cli.sh` itself needs
  `RUSTFLAGS='-C target-feature=+crt-static'` on Windows.
