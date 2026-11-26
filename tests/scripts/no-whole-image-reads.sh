#!/usr/bin/env bash
#
# no-whole-image-reads.sh — a test never reads a disk image into memory (#450).
#
# libtest runs one test per CPU, so whole-file reads of images multiply by the
# number of tests running. tests/support/bounded.rs compares files one mebibyte
# at a time, and its `small_file` reads a file whole under a 1 MiB limit. So a
# direct whole-file read anywhere else under tests/ is refused. One is reviewed:
# the receipt `hdiutil attach` writes, in tests/support/hdiutil_attachment.rs,
# which is a few lines of text.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
PATTERN='(^|[^[:alnum:]_])fs::read[[:space:]]*\('
REVIEWED='tests/support/hdiutil_attachment.rs'

# The Rust files under tests/ that call it, one `path:count` a line.
callers() {
    local file count
    find "$1/tests" -name '*.rs' -type f | sort | while IFS= read -r file; do
        count="$(grep -Ec "$PATTERN" "$file" || true)"
        [ "$count" = 0 ] || printf '%s:%s\n' "${file#"$1"/}" "$count"
    done
}

# The guard has to be able to fail.
for line in 'let bytes = std::fs::read(&image);' 'fs::read(path)'; do
    if ! grep -Eq "$PATTERN" <<<"$line"; then
        echo "FAIL  the guard does not see a whole-file read in: $line" >&2
        exit 1
    fi
done
if grep -Eq "$PATTERN" <<<'std::fs::read_to_string(&dump)'; then
    echo "FAIL  the guard takes read_to_string for a whole-file read" >&2
    exit 1
fi

found="$(callers "$REPO")"
if [ "$found" != "$REVIEWED:1" ]; then
    echo "FAIL  whole-file reads under tests/ differ from the one reviewed ($REVIEWED, once):" >&2
    printf '%s\n' "${found:-(none)}" >&2
    echo "      read a file through tests/support/bounded.rs" >&2
    exit 1
fi
echo "PASS  no test reads a disk image into memory"
