#!/usr/bin/env bash
# guest-rust-toolchain.sh -- install the calling repository's pinned Rust
# toolchain into a test VM's guest, recovering from an install that was
# interrupted. Run it as `scripts/core.sh guest-rust-toolchain`, inside the
# guest; this file is the only copy.
#
#   RUSTUP_HOME=/var/lib/fs-x-rust/rustup CARGO_HOME=/var/lib/fs-x-rust/cargo \
#       scripts/core.sh guest-rust-toolchain
#
# THE TOOLCHAIN THE REPOSITORY PINS, AND ONLY THAT ONE: the channel in the
# caller's rust-toolchain.toml, with rustfmt and clippy on the minimal
# profile, made the default. A guest that silently built with a different
# compiler than CI is a guest whose result means nothing.
#
# RUSTUP_HOME AND CARGO_HOME ARE REQUIRED, not defaulted. They name the
# guest's own disk, which outlives `vm:down`, so the second run is quick;
# and a script that fell back to ~/.rustup would rewrite a developer's own
# toolchain if it were ever run on a host.
#
# AN INSTALL THAT WAS INTERRUPTED -- a reaper or a deadline stopping the VM
# in the middle of it, a laptop closing -- leaves wreckage on that same disk,
# and rustup will not clear it. Every later run then fails the same way until
# somebody destroys the VM. Three kinds were measured on a guest
# (antimatter-studios/rust-fs-core#190, first fixed in rust-fs-xfs alone):
#
#   - downloads/ holds a `.partial` whose final name rustup cannot produce:
#     "could not rename 'downloaded' file ... No such file or directory".
#     downloads/ and tmp/ are caches, so they are cleared before every run;
#     that costs a re-download and nothing else.
#   - a toolchain directory half populated: "detected conflict:
#     'bin/rust-gdb'", or "could not rename 'component' file ... Directory
#     not empty". When the install fails, the toolchain is removed and
#     installed once more. rustup is idempotent and quick over a whole
#     toolchain, so the ordinary case pays nothing for this.
#   - a rustup that rustup-init never finished writing. One that cannot
#     answer --version is fetched again.
#
# TWO SUITES CAN SHARE ONE GUEST (the harness keys a VM by project, so every
# worktree of a repository runs in the same one), and two installs in one
# RUSTUP_HOME clear each other's downloads. The whole install holds a lock.
#
# Needs curl (rustup-init) and flock (util-linux) in the guest.
set -euo pipefail

GUEST_RUST_TOOLCHAIN_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-guest-rust-toolchain %s\n' "$GUEST_RUST_TOOLCHAIN_API_VERSION"
    exit 0
fi
[ $# -eq 0 ] || { echo "usage: scripts/core.sh guest-rust-toolchain" >&2; exit 2; }

die() {
    echo "guest-rust-toolchain: $*" >&2
    exit 1
}

CALLER="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

[ -n "${RUSTUP_HOME:-}" ] || die "RUSTUP_HOME is not set; name a directory on the guest's own disk."
[ -n "${CARGO_HOME:-}" ] || die "CARGO_HOME is not set; name a directory on the guest's own disk."
export RUSTUP_HOME CARGO_HOME

pin="$CALLER/rust-toolchain.toml"
[ -f "$pin" ] || die "$pin does not exist; it is what names the toolchain."
toolchain="$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$pin" | sed -n 1p)"
[ -n "$toolchain" ] || die "$pin pins no channel."

for tool in curl flock; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed in the guest."
done

mkdir -p "$RUSTUP_HOME" "$CARGO_HOME"
exec 9>"$RUSTUP_HOME/.guest-rust-toolchain.lock"
flock 9

rustup="$CARGO_HOME/bin/rustup"

rm -rf "$RUSTUP_HOME/downloads" "$RUSTUP_HOME/tmp"

if ! "$rustup" --version >/dev/null 2>&1; then
    rm -f "$rustup"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --no-modify-path --default-toolchain none >/dev/null
    "$rustup" --version >/dev/null 2>&1 || die "rustup-init did not leave a working $rustup."
fi

install_toolchain() {
    "$rustup" toolchain install "$toolchain" \
        --component rustfmt --component clippy --profile minimal
}

if ! first="$(install_toolchain 2>&1)"; then
    echo "guest-rust-toolchain: $toolchain did not install; removing what is there and trying once more." >&2
    printf '%s\n' "$first" | tail -n 5 | sed 's/^/    /' >&2
    rm -rf "$RUSTUP_HOME/toolchains/$toolchain"* "$RUSTUP_HOME/update-hashes/$toolchain"* \
        "$RUSTUP_HOME/downloads" "$RUSTUP_HOME/tmp"
    install_toolchain >/dev/null
fi
"$rustup" default "$toolchain" >/dev/null
cargo="$("$rustup" run "$toolchain" cargo --version)" ||
    die "$toolchain installed, but its cargo does not run."
echo "guest-rust-toolchain: $cargo"
