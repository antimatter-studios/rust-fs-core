#!/usr/bin/env bash
# changelog-draft.sh [--write] -- a draft CHANGELOG entry from the commits
# since the newest tag (#209). Run it as `scripts/core.sh changelog-draft`.
#
# Each squash commit on the caller's history since the newest `v*` tag
# becomes one bullet, grouped by the commit's conventional type:
#
#   feat:               -> ### Added
#   fix:                -> ### Fixed
#   anything else       -> ### Changed
#   chore(release): ... -> skipped (it is the release, not a change)
#
# A subject like `fix: a short read is refused (#22)` becomes
# `- A short read is refused ([#22](https://github.com/OWNER/REPO/pull/22)).`
#
# Without --write the draft is printed. With --write it is put under
# `## [Unreleased]` in $FS_CORE_CALLER/CHANGELOG.md, above whatever is already
# there. Either way it is a STARTING POINT: the family's CHANGELOG explains a
# breaking or user-visible change in prose a commit subject cannot hold, and
# that is still written by hand before the release.
set -euo pipefail

CHANGELOG_DRAFT_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-changelog-draft %s\n' "$CHANGELOG_DRAFT_API_VERSION"
    exit 0
fi
write=0
case "${1:-}" in
    "") ;;
    --write) write=1 ;;
    *) echo "usage: changelog-draft.sh [--write]" >&2; exit 2 ;;
esac

ROOT="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
cd "$ROOT"

repo="${GITHUB_REPOSITORY:-}"
if [ -z "$repo" ]; then
    url="$(git remote get-url origin 2>/dev/null || true)"
    repo="$(printf '%s' "$url" | sed -E 's#^(git@github\.com:|https://github\.com/)##; s#\.git$##')"
fi

tag="$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)"
range="${tag:+$tag..}HEAD"

draft="$(git log --format='%s' "$range" | REPO="$repo" python3 -c '
import os, re, sys
repo = os.environ.get("REPO", "")
groups = {"Added": [], "Fixed": [], "Changed": []}
for line in sys.stdin:
    s = line.strip()
    if not s:
        continue
    m = re.match(r"^(\w+)(\([^)]*\))?(!)?:\s*(.*)$", s)
    kind, scope, text = (m.group(1), m.group(2) or "", m.group(4)) if m else ("", "", s)
    if kind == "chore" and scope == "(release)":
        continue
    pr = re.search(r"\s*\(#(\d+)\)\s*$", text)
    if pr:
        text = text[:pr.start()]
        ref = f" ([#{pr.group(1)}](https://github.com/{repo}/pull/{pr.group(1)}))" if repo else f" (#{pr.group(1)})"
    else:
        ref = ""
    text = text.strip().rstrip(".")
    if not text:
        continue
    text = text[0].upper() + text[1:]
    group = "Added" if kind == "feat" else "Fixed" if kind == "fix" else "Changed"
    groups[group].append(f"- {text}{ref}.")
out = []
for name in ("Added", "Fixed", "Changed"):
    if groups[name]:
        out.append(f"### {name}\n\n" + "\n".join(reversed(groups[name])) + "\n")
print("\n".join(out), end="")
')"

if [ -z "$draft" ]; then
    echo "changelog-draft: no changes since ${tag:-the first commit}." >&2
    exit 0
fi

if [ "$write" = 0 ]; then
    printf '%s\n' "$draft"
    exit 0
fi

[ -f CHANGELOG.md ] || { echo "changelog-draft: CHANGELOG.md does not exist" >&2; exit 1; }
grep -q '^## \[Unreleased\]' CHANGELOG.md || {
    echo "changelog-draft: CHANGELOG.md has no '## [Unreleased]' section to write under" >&2
    exit 1
}
DRAFT="$draft" python3 - <<'PY'
import os, re
p = "CHANGELOG.md"
s = open(p).read()
m = re.search(r"^## \[Unreleased\][^\n]*\n", s, flags=re.M)
s = s[:m.end()] + "\n" + os.environ["DRAFT"].rstrip("\n") + "\n" + s[m.end():]
open(p, "w").write(s)
PY
echo "changelog-draft: written under [Unreleased] in CHANGELOG.md; edit it before the release." >&2
