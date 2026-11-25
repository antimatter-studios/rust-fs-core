#!/usr/bin/env bash
# Copy the static archive from Cargo's resolved target directory.
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 TARGET LIBRARY OUTPUT-DIR" >&2
    exit 2
fi

triple="$1"
library="$2"
out="$3"
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
cp "$target_dir/$triple/release/lib$library.a" "$out/lib$library.a"
