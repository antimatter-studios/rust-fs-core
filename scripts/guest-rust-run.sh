#!/usr/bin/env bash
# guest-rust-run.sh -- inside a test VM, set up the Rust layer every driver's
# suite needs and then run the suite. Run it as `scripts/core.sh
# guest-rust-run`, from the driver's [test] guest_command; this file is the
# only copy.
#
#   scripts/core.sh guest-rust-run NAME SHARE SIBLING... -- COMMAND [ARG...]
#
#   FS_CORE_ROOT=/share/siblings/rust-fs-core exec bash scripts/core.sh \
#       guest-rust-run fs-xyz /share rust-fs-core -- scripts/test.sh --locked --release "$@"
#
# IN ORDER, refusing at the first thing that is not right:
#
#   1. IT IS IN THE VM. The harness exports FLTH_GUEST=1 to what it runs in
#      the guest. Anywhere else this would link into the directory that
#      holds the caller -- on a host, a developer's own -- so it refuses
#      first, touching nothing.
#   2. THE SIBLINGS ARE LINKED. The caller's path dependencies name
#      ../SIBLING, and the guest is given the repository, not the directory
#      that holds it. Each sibling `scripts/core.sh stage-siblings` put at
#      SHARE/siblings/SIBLING on the host is linked at ../SIBLING, beside
#      the caller -- in the harness's guest, /repo's parent is /. A link
#      left pointing elsewhere is pointed back; a real directory in its
#      place is refused, not replaced.
#   3. THE TOOLCHAIN AND THE BUILD ARE ON THE GUEST'S OWN DISK, which
#      outlives `vm:down`, so a second run is incremental:
#      RUSTUP_HOME and CARGO_HOME under /var/lib/NAME-rust, CARGO_TARGET_DIR
#      at /var/cache/NAME-target, cargo first on PATH. NAME keeps two
#      drivers' builds apart. FS_CORE_GUEST_VAR moves /var, for tests.
#   4. THE PINNED TOOLCHAIN IS INSTALLED, through this repository's
#      guest-rust-toolchain.sh, which recovers from an install that was
#      interrupted (#190).
#   5. THE SUITE RUNS, from the caller, with its arguments as given, between
#      a line naming the kernel and the cargo and a line with the time it
#      took. Its status is the run's.
#
# THE ONE THING IT CANNOT DO is be found before core is in the guest:
# scripts/core.sh looks for ../rust-fs-core, which is what step 2 creates.
# So the caller names the staged copy with FS_CORE_ROOT, as above.
#
# Five drivers carried steps 1-3 and 5 at the head of their
# scripts/guest-suite.sh, each differing only in NAME (#195).
set -euo pipefail

GUEST_RUST_RUN_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-guest-rust-run %s\n' "$GUEST_RUST_RUN_API_VERSION"
    exit 0
fi

usage() {
    echo "usage: scripts/core.sh guest-rust-run NAME SHARE SIBLING... -- COMMAND [ARG...]" >&2
    [ $# -eq 0 ] || echo "guest-rust-run: $*" >&2
    exit 2
}
die() {
    echo "guest-rust-run: $*" >&2
    exit 1
}

[ $# -ge 2 ] || usage
NAME="$1"
SHARE="$2"
shift 2
case "$NAME" in
    "" | . | .. | *[!A-Za-z0-9._-]*) usage "'$NAME' is not a name for a directory under /var." ;;
esac
[ -n "$SHARE" ] || usage "the share is an empty string."
siblings=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    case "$1" in
        "" | . | .. | */*) usage "'$1' is not a sibling's directory name." ;;
    esac
    siblings+=("$1")
    shift
done
[ "${#siblings[@]}" -gt 0 ] || usage "name at least one sibling."
[ $# -gt 0 ] || usage "the suite to run follows --."
shift
[ $# -gt 0 ] || usage "nothing follows --."

# 1. In the VM.
[ "${FLTH_GUEST:-}" = 1 ] ||
    die "FLTH_GUEST is not 1: this runs INSIDE the harness VM ('chore test:vm'), and touches nothing here."

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CALLER="${FS_CORE_CALLER:-$(cd "$HERE/.." && pwd)}"
PARENT="$(dirname "$CALLER")"

# 2. The siblings, every one checked before any is linked.
for sibling in "${siblings[@]}"; do
    [ -d "$SHARE/siblings/$sibling" ] ||
        die "$sibling is not staged at $SHARE/siblings/$sibling; 'scripts/core.sh stage-siblings', in 'chore test:vm', does that."
    link="$PARENT/$sibling"
    if [ -e "$link" ] && [ ! -L "$link" ]; then
        die "$link is there and is not a link; it is where the staged $sibling is linked, and it is left alone."
    fi
done
for sibling in "${siblings[@]}"; do
    staged="$SHARE/siblings/$sibling"
    link="$PARENT/$sibling"
    if [ ! -L "$link" ] || [ "$(readlink "$link")" != "$staged" ]; then
        ln -sfn "$staged" "$link"
    fi
done

# 3. The guest's own disk.
VAR="${FS_CORE_GUEST_VAR:-/var}"
RUST_ROOT="$VAR/lib/$NAME-rust"
export RUSTUP_HOME="$RUST_ROOT/rustup"
export CARGO_HOME="$RUST_ROOT/cargo"
export CARGO_TARGET_DIR="$VAR/cache/$NAME-target"
export PATH="$CARGO_HOME/bin:$PATH"

# 4. The pinned toolchain.
FS_CORE_CALLER="$CALLER" bash "$HERE/guest-rust-toolchain.sh"
command -v cargo >/dev/null 2>&1 ||
    die "there is no cargo on PATH after the toolchain install."

# 5. The suite.
cd "$CALLER"
echo "== in-guest suite: $(uname -srm), $(cargo --version)"
started=$(date +%s)
status=0
"$@" || status=$?
echo "== in-guest suite: $(($(date +%s) - started))s"
exit "$status"
