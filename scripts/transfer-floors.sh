#!/usr/bin/env bash
# transfer-floors.sh TIER -- the tier's transfer targets met the floors every
# host has, then the host OS's own, as ci.yml checks them. chores.yml and
# release.yml run it, so a local or release tier accepts no less than CI (#516).
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: transfer-floors.sh TIER" >&2
    exit 2
fi
tier="$1"
host="$(uname -s)"
case "$host" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    MINGW* | MSYS*) os=windows ;;
    *)
        echo "transfer-floors.sh: no per-OS transfer floors for $host" >&2
        exit 1
        ;;
esac
core="$(dirname "$0")/core.sh"
bash "$core" test-floor --targets .github/transfer-test-floors.txt "$tier"
bash "$core" test-floor --targets ".github/transfer-$os-test-floors.txt" "$tier"
