#!/usr/bin/env bash
# release-notes.sh VERSION -- a release's notes: the caller's CHANGELOG
# section for VERSION (#209). Run it as `scripts/core.sh release-notes
# VERSION` from any repository in the family; this file is the only copy.
#
# Prints, to stdout:
#
#   - the body of the `## [VERSION]` (or `## vVERSION`) section of $FS_CORE_CALLER/CHANGELOG.md,
#     without its heading (the release is already titled with the tag);
#   - a link to the diff from the previous version the CHANGELOG lists;
#   - the provenance line every release in the family carries.
#
# A version the CHANGELOG does not describe is REFUSED: exit 1, nothing on
# stdout, the reason on stderr. A release workflow that took an empty body
# would publish the "See CHANGELOG.md" page this script exists to replace,
# so the release stops instead and the section gets written.
#
# The repository the links point at is $GITHUB_REPOSITORY (set in every
# GitHub Actions job), else the caller's `origin` remote.
set -euo pipefail

RELEASE_NOTES_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-release-notes %s\n' "$RELEASE_NOTES_API_VERSION"
    exit 0
fi
# --unattested: the release attaches files its workflow built but does not
# attest, so the provenance line says that rather than claim an attestation.
unattested=0
if [ "${1:-}" = "--unattested" ]; then unattested=1; shift; fi
[ $# -eq 1 ] || { echo "usage: release-notes.sh [--unattested] VERSION" >&2; exit 2; }
version="${1#v}"

ROOT="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
changelog="$ROOT/CHANGELOG.md"
[ -f "$changelog" ] || { echo "release-notes: $changelog does not exist" >&2; exit 1; }

repo="${GITHUB_REPOSITORY:-}"
if [ -z "$repo" ]; then
    url="$(git -C "$ROOT" remote get-url origin 2>/dev/null || true)"
    repo="$(printf '%s' "$url" | sed -E 's#^(git@github\.com:|https://github\.com/)##; s#\.git$##')"
fi

# A section's version is the first word of its `## ` heading, in any of the
# family's styles: `## [X.Y.Z]` (Keep a Changelog), `## vX.Y.Z` or `## X.Y.Z`.
# Any `## ` heading ends the section before it.
HEADING_VERSION='function heading_version(line,  h) { split(line, w, /[ \t]+/); h = w[2]; sub(/^\[/, "", h); sub(/\].*$/, "", h); sub(/^v/, "", h); return h }'

body="$(awk -v v="$version" "$HEADING_VERSION"'
    /^## / {
        if (inside) exit
        if (heading_version($0) == v) { inside = 1; next }
    }
    inside { print }
' "$changelog")"
# Trim the blank lines around the section.
body="$(printf '%s\n' "$body" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"

if [ -z "$(printf '%s' "$body" | tr -d '[:space:]')" ]; then
    echo "release-notes: CHANGELOG.md has no section for $version (a '## [$version]', '## v$version' or '## $version' heading with text under it)." >&2
    echo "               Write it before tagging: a release with no notes is not published." >&2
    exit 1
fi

previous="$(awk -v v="$version" "$HEADING_VERSION"'
    /^## / {
        h = heading_version($0)
        if (h !~ /^[0-9]/) next
        if (found) { print h; exit }
        if (h == v) found = 1
    }
' "$changelog")"

printf '%s\n' "$body"
printf '\n---\n\n'
if [ -n "$repo" ] && [ -n "$previous" ]; then
    printf '**Changes since %s:** https://github.com/%s/compare/v%s...v%s\n\n' "$previous" "$repo" "$previous" "$version"
fi
if [ "$unattested" = 1 ]; then
    printf 'The files attached here are the ones this tag'"'"'s release workflow built.\n'
else
    printf 'The files attached here are the ones this tag published, each with a build-provenance attestation from this repository'"'"'s release workflow.\n'
fi
