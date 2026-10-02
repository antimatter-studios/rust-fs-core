#!/usr/bin/env bash
# semver-check.sh -- refuse a public-API break the version does not declare.
# Run it as `scripts/core.sh semver-check` from any crate in the family; this
# file is the only copy. It checks the CALLER's crate: the package named in
# $FS_CORE_CALLER/Cargo.toml (scripts/core.sh sets it; run directly, the crate
# is the one this file is in).
#
# Compares the crate's public API against the newest version PUBLISHED TO
# CRATES.IO -- not a tag, not origin/main -- with cargo-semver-checks, and fails
# when the change needs a bigger bump than Cargo.toml's version makes. For a
# 0.x crate a break needs the minor to move (0.4.0 -> 0.5.0); an addition
# needs at least the patch. While Cargo.toml stays on the published version
# the check assumes a minor bump, so additions pass and breaks fail.
#
# WHY THIS EXISTS. A break published as a patch breaks every `^0.x` consumer's
# build on its next `cargo update`, and the only thing standing in the way was
# a person remembering to bump the version. In one week two crates had breaks
# sitting on main under a version already published: a new `Error` variant in
# antimatter-studios/rust-fs-xfs#292, caught only by a hand bump in the release
# PR, and two Rust-API breaks in christhomas/rust-fs-ntfs#399. rust-fs-ext4 met
# the shape first (christhomas/rust-fs-ext4#120).
#
# THE BASELINE IS THE REGISTRY because that is what a consumer has. A tag can
# exist for a version that never published, and a version can publish from a
# commit no tag names; the registry is the one list a downstream `cargo update`
# actually reads.
#
# WHAT IT CANNOT SEE. A change of behaviour behind an unchanged signature, and
# anything in the C ABI: cargo-semver-checks reads Rust items, not a C
# struct's layout or an exported function's arity as a C caller sees it. Those
# still need a changelog line written by a person.
#
# cargo-semver-checks is MIT/Apache-2.0. CARGO_SEMVER_CHECKS_VERSION pins the
# version CI installs; a different local one is reported, not refused.
# SEMVER_CHECK_DRY_RUN=1 prints the command instead of running it.
set -euo pipefail

SEMVER_CHECK_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-semver-check %s\n' "$SEMVER_CHECK_API_VERSION"
    exit 0
fi
[ $# -eq 0 ] || { echo "usage: semver-check.sh" >&2; exit 2; }

ROOT="${FS_CORE_CALLER:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$ROOT"

# [package]'s own name and version: the first `name =` / `version =` after the
# [package] header and before the next table.
read_package() {
    awk -v key="$1" '
        /^\[/ { in_pkg = ($0 == "[package]") ; next }
        in_pkg && $1 == key && $2 == "=" { gsub(/"/, "", $3); print $3; exit }
    ' Cargo.toml
}
crate="$(read_package name)"
version="$(read_package version)"
[ -n "$crate" ] || { echo "semver-check: $ROOT/Cargo.toml names no [package]" >&2; exit 1; }

command=(cargo semver-checks check-release --package "$crate")
if [ "${SEMVER_CHECK_DRY_RUN:-0}" = 1 ]; then
    echo "semver-check: would run in $ROOT: ${command[*]}"
    exit 0
fi

PINNED="${CARGO_SEMVER_CHECKS_VERSION:-0.50.0}"
if ! cargo semver-checks --version >/dev/null 2>&1; then
    echo "semver-check: cargo-semver-checks is not installed." >&2
    echo "              cargo install cargo-semver-checks --locked --version $PINNED" >&2
    exit 1
fi
have="$(cargo semver-checks --version | awk '{print $2}')"
if [ "$have" != "$PINNED" ]; then
    echo "semver-check: note: cargo-semver-checks $have here, CI pins $PINNED" >&2
fi

echo "semver-check: $crate $version against the newest crates.io release"
# --release-type is NOT passed: the bump is read from Cargo.toml, so the
# version a release would publish is the version being checked.
exec "${command[@]}"
