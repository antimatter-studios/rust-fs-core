#!/usr/bin/env bash
# stage-siblings.sh -- put the calling repository's path siblings on a test
# VM's share, at their committed HEAD, and then run the guest command. Run
# it as `scripts/core.sh stage-siblings`, on the host; this file is the only
# copy.
#
#   scripts/core.sh stage-siblings SHARE SIBLING... [-- COMMAND [ARG...]]
#
#   share="$(../fs-linux-test-harness/scripts/vm.sh share)"
#   scripts/tier.sh test:vm vm LINES BYTES -- \
#       bash scripts/core.sh stage-siblings "$share" rust-fs-core -- \
#       ../fs-linux-test-harness/scripts/vm.sh guest-test
#
# WHY. A driver's path dependency on ../rust-fs-core resolves on the host,
# where the sibling is checked out beside it. The test VM is given the
# repository and not the directory that holds it, so in there the path has
# nothing to resolve to until the sibling is put on the share, which both
# sides see; `scripts/core.sh guest-rust-run` links it into place inside.
# This half has to run on the host, before the guest exists, so it cannot be
# anything the guest is given.
#
# EACH SIBLING IS STAGED FROM ITS HEAD, through `git archive`, from the
# checkout at ../SIBLING: what the guest builds against is what `chore
# siblings` pinned, and nothing uncommitted, untracked or of git's own. It
# goes to SHARE/siblings/SIBLING, and whatever was staged there before is
# removed first, so a file the sibling no longer has does not linger.
#
# THE COMMAND, when one follows `--`, runs once every sibling is staged, in
# the caller, with the words chore was given (CLI_ARGS) appended -- less
# `--verbose` and `-v`. Those are the tier wrapper's: it reads them out of
# CLI_ARGS and streams, and passed on to the guest's `cargo test` they would
# mean something else entirely. A staging that fails runs no command.
#
# Five drivers carried this inline in their `test:vm` task (#195).
set -euo pipefail

STAGE_SIBLINGS_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-stage-siblings %s\n' "$STAGE_SIBLINGS_API_VERSION"
    exit 0
fi

usage() {
    echo "usage: scripts/core.sh stage-siblings SHARE SIBLING... [-- COMMAND [ARG...]]" >&2
    [ $# -eq 0 ] || echo "stage-siblings: $*" >&2
    exit 2
}
die() {
    echo "stage-siblings: $*" >&2
    exit 1
}

CALLER="${FS_CORE_CALLER:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

[ $# -ge 1 ] || usage
SHARE="$1"
shift
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
command=()
if [ $# -gt 0 ]; then
    shift
    [ $# -gt 0 ] || usage "nothing follows --."
    command=("$@")
fi

# Checked as given, and never created: a share that is not there is a
# harness that is not up, and an empty one would stage into /siblings.
[ -d "$SHARE" ] || die "the share $SHARE is not a directory."

for sibling in "${siblings[@]}"; do
    source="$CALLER/../$sibling"
    [ -e "$source/.git" ] ||
        die "../$sibling is not checked out beside $CALLER -- run 'chore siblings'."
    git -C "$source" rev-parse --verify --quiet HEAD >/dev/null ||
        die "../$sibling has no commit to stage."
    dest="$SHARE/siblings/$sibling"
    rm -rf "$dest"
    mkdir -p "$dest"
    git -C "$source" archive --format=tar HEAD | tar -x -C "$dest" ||
        die "../$sibling did not stage onto $dest."
done

[ "${#command[@]}" -gt 0 ] || exit 0

# chore's CLI_ARGS is one string; its words are split, and not globbed.
read -ra words <<<"${CLI_ARGS:-}"
args=()
for arg in ${words[@]+"${words[@]}"}; do
    case "$arg" in --verbose | -v) continue ;; esac
    args+=("$arg")
done
cd "$CALLER"
exec "${command[@]}" ${args[@]+"${args[@]}"}
