#!/usr/bin/env bash
# Rebuild fuzz/corpus.
#
# THIS CORPUS IS NOT CUT FROM AN IMAGE, unlike every other repository in
# the constellation, because nothing here parses one. The inputs are
# read as geometry -- a slice's start and length, and a read's offset
# and length -- so the seeds are the tuples worth starting from: every
# boundary a bounds check can be off by one about.
#
# Usage: scripts/make-fuzz-corpus.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rm -rf "$here/fuzz/corpus"
mkdir -p "$here/fuzz/corpus"/{slice,cache}

python3 - "$here/fuzz/corpus" <<'PY'
import os, struct, sys

root = sys.argv[1]
PARENT_LEN = 4096

# start, length, offset, read_len -- the interesting tuples rather than
# random ones. A fuzzer will find its own; these are the ones a person
# would write down, and the ones a regression is most likely to be.
cases = {
    'whole-parent':        (0, PARENT_LEN, 0, PARENT_LEN),
    'empty-read':          (0, PARENT_LEN, 0, 0),
    'last-byte':           (0, PARENT_LEN, PARENT_LEN - 1, 1),
    'one-past-the-end':    (0, PARENT_LEN, PARENT_LEN, 1),
    'ends-exactly-at-end': (16, 64, 0, 64),
    'one-past-slice-end':  (16, 64, 0, 65),
    'slice-past-parent':   (PARENT_LEN - 8, 64, 0, 64),
    'start-at-parent-end': (PARENT_LEN, 64, 0, 1),
    'zero-length-slice':   (128, 0, 0, 1),
    'offset-overflows':    (1 << 63, 1 << 63, 1 << 63, 8),
    'length-overflows':    (8, (1 << 64) - 1, 0, 16),
    'unaligned-straddle':  (3, 4000, 1021, 2050),
    # rust-fs-core#151: the read whose END does not fit in a u64.
    #
    # NOT COVERED BY 'offset-overflows' ABOVE, which is named for the
    # slice and only overflows `start + offset`. Its cache geometry is
    # offset 2**63 reading 8 bytes, and 2**63 + 8 fits perfectly well --
    # so before this entry no committed seed made `offset + buf.len()`
    # wrap, which is the sum that panicked.
    #
    # THE OFFSET MUST STAY A MULTIPLE OF 8. `geometry_from`'s `near()`
    # folds anything else into the parent's range to keep most cases at
    # the edges, and a folded offset is an ordinary small one: the seed
    # would still be here, still be named for the wrap, and no longer
    # produce it. `the_wrapping_read_seed_still_wraps` is what notices.
    #
    # start 4 and length 511 are not spare: the cache target reads them
    # as capacity `4 % 8` and block size `1 + 511 % 1024`, so this runs
    # against a real 512-byte-block cache rather than a degenerate one.
    'read-end-wraps':      (4, 511, (1 << 64) - 8, 16),
}

for name, (start, length, offset, read_len) in cases.items():
    blob = struct.pack('<QQQQ', start, length, offset, read_len)
    for kind in ('slice', 'cache'):
        with open(os.path.join(root, kind, f'{name}.bin'), 'wb') as f:
            f.write(blob)
PY

echo "corpus rebuilt under fuzz/corpus:"
find "$here/fuzz/corpus" -type f | sort | sed "s#$here/##"
echo "total: $(find "$here/fuzz/corpus" -type f | wc -l) seeds, $(du -sh "$here/fuzz/corpus" | cut -f1)"
