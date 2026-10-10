#!/usr/bin/env bash
# package-cli.sh VERSION LABEL [TARGET-DIR] -- package the calling
# repository's command-line tools as a release tarball in the current
# directory, check it, and print its file name on stdout. Run it as
# `scripts/core.sh package-cli VERSION LABEL [TARGET-DIR]`; this file is the
# only copy.
#
#   VERSION     the release version, without the leading `v`
#   LABEL       the platform, e.g. darwin-arm64 or linux-x86_64; a label
#               starting windows- packages for Windows (below)
#   TARGET-DIR  where cargo put the release build (default: the caller's
#               target/release); a relative path is taken from the current
#               directory, which is also where the tarball is written
#
# Each repository once carried its own copy of this script, and the ten
# copies drifted into three policies for which names go in the tarball: one
# never moved to the multi-call layout and shipped a single tool. There is
# one policy now, and a repository says only what is its own, in its
# Cargo.toml:
#
#   [package.metadata.package-cli]
#   names = { "mkfs.example" = 8, "fs.example" = 1 }   # every dotted name,
#                                                      # and its man section
#   licenses = ["LICENSE"]                 # files at the repository's root
#   caveats = "packaging/CAVEATS"          # optional; this is the default
#
# The crate's name comes from `name`, and <repo> -- the binary, which must be
# a [[bin]] target of that name -- from the last segment of `repository`.
# Cargo reads the manifest (`cargo metadata --no-deps`), so this script
# parses no TOML; python3 reads cargo's JSON.
#
# THE TARBALL IS THE CONTRACT with whatever installs it. It is an install
# prefix, the same in every repository, so an installer copies it as-is and
# needs to know nothing about which tools are in it:
#
#   bin/<repo>                               the multi-call binary, the real file
#   bin/<name> -> <repo>                     each dotted name, a RELATIVE symlink
#   share/man/man<S>/<name>.<S>              a page per name in its section S,
#   share/man/man<S>/<name>-<verb>.<S>       and one per subcommand beside it
#   share/zsh/site-functions/_<name>         a completion per name, per shell
#   share/bash-completion/completions/<name>
#   share/fish/vendor_completions.d/<name>.fish
#   share/<repo>/CAVEATS                     at most four lines an installer shows
#   <licence files>
#
# where <name> is each dotted name and <repo> itself, whose page is section
# 1.
#
# ON WINDOWS (a windows- LABEL) the archive is <crate>-<version>-<label>.zip,
# and bin/ holds bin/<repo>.exe, the binary cargo wrote, and each dotted name
# as a byte-identical COPY, bin/<name>.exe: a symlink needs Developer Mode or
# an administrator there, and a zip holds no symlink anyway. The binary
# answers as the name it was started under with .exe dropped
# (rust-fs-core's cli::dispatch::invoked_name), so a copy is as good as a
# link. Everything under share/ and the licences are the same as on every
# other platform. Python's zipfile writes and reads the zip on every host,
# because neither zip nor unzip is on a Windows runner (#232). The pages and completions are written by the binary (`<repo> generate
# man|completions SHARE`, rust-fs-core's `cli` module), from the clap commands
# it parses with, so they cannot describe a flag it does not take.
#
# THE NAMES ARE WRITTEN DOWN, in Cargo.toml, AND READ FROM THE BINARY
# (`<repo> generate names`), and the two must agree as sets: a binary that
# forgot a name cannot agree with itself, and a name nobody declared is not
# shipped by accident. Cargo refuses a dot in a target name, so the dotted
# names exist only as the symlinks made here.
#
# THEN IT CHECKS WHAT IT BUILT, from the unpacked tarball, because a tarball
# whose tools do not run is worse than no tarball: the failure would surface
# as a user's bug report rather than a red release. Exactly the members
# above; every dotted name a relative symlink to bin/<repo> (on Windows, a
# copy of bin/<repo>.exe); a page in its
# declared section and three completions for every name; CAVEATS at most
# four lines; the licences and CAVEATS the repository's own; and every name
# answering --help, and --version as `<name> (<crate>) <version>`, which
# identifies it among same-named tools from other packages and catches a tag
# that disagrees with Cargo.toml. On any failure nothing is printed on stdout
# and no tarball is left behind, not even one a previous run wrote.
#
# EVERY LIST IS COMPARED IN THE C LOCALE. A bare `sort` collates by the
# caller's locale, which put LICENSE after bin/ on a developer's machine and
# before it on CI; the copies' tests failed on correct tarballs that way in
# eight repositories. LC_ALL=C is set once, here, for everything below.
# tests/package_cli.rs holds this script to all of it, against caller trees.
set -euo pipefail
export LC_ALL=C

PACKAGE_CLI_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-package-cli %s\n' "$PACKAGE_CLI_API_VERSION"
    exit 0
fi

die() { echo "package-cli: $*" >&2; exit 1; }
usage="usage: scripts/core.sh package-cli VERSION LABEL [TARGET-DIR]"
[ $# -ge 2 ] && [ $# -le 3 ] || die "$usage"
version="$1"
label="$2"
[ -n "$version" ] || die "no VERSION; $usage"
[ -n "$label" ] || die "no LABEL; $usage"
case "$version$label" in
    */*) die "VERSION and LABEL name a file; '$version' and '$label' cannot hold a /" ;;
esac

CORE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CALLER="$(cd "${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}" && pwd)"
target_dir="${3:-$CALLER/target/release}"

# ---- What the caller ships, from its Cargo.toml. ---------------------------
command -v cargo >/dev/null 2>&1 || die "cargo is needed to read $CALLER/Cargo.toml"
# python3, or `python` where that is what Python 3 is called, as on a
# Windows runner. Each is run, not merely found: Windows answers `python3`
# with a stub that opens the Store.
PYTHON=""
for candidate in python3 python; do
    if "$candidate" -c 'import sys; sys.exit(sys.version_info[0] != 3)' >/dev/null 2>&1; then
        PYTHON="$candidate"
        break
    fi
done
[ -n "$PYTHON" ] || die "python3 is needed to read cargo's metadata"
[ -f "$CALLER/Cargo.toml" ] || die "no Cargo.toml in $CALLER"
metadata="$(cargo metadata --no-deps --offline --format-version 1 \
    --manifest-path "$CALLER/Cargo.toml")" || die "cargo could not read $CALLER/Cargo.toml"

# One line per fact, tab-separated, for the shell to read: `crate NAME`,
# `repo NAME`, `caveats PATH`, `licence FILE`..., `name NAME SECTION`....
# Without the CR Python on Windows ends each line with: `read` would keep it,
# and the binary would be looked for as `<repo>\r.exe`.
config="$(printf '%s' "$metadata" | "$PYTHON" -c '
import json, os, re, sys

manifest = os.path.realpath(sys.argv[1])
def fail(why):
    sys.stderr.write("package-cli: %s: %s\n" % (manifest, why))
    sys.exit(1)

packages = [p for p in json.load(sys.stdin)["packages"]
            if os.path.realpath(p["manifest_path"]) == manifest]
if len(packages) != 1:
    fail("is not a package")
package = packages[0]

repo = (package.get("repository") or "").rstrip("/")
repo = repo[:-4] if repo.endswith(".git") else repo
repo = repo.rsplit("/", 1)[-1]
if not repo:
    fail("has no `repository`, and the binary and share/<repo>/ are named for it")
bins = sorted(t["name"] for t in package["targets"] if "bin" in t["kind"])
if repo not in bins:
    fail("has no [[bin]] named %r, the multi-call binary the tarball ships (its bins: %s)"
         % (repo, ", ".join(bins) or "none"))

table = (package.get("metadata") or {}).get("package-cli")
if not isinstance(table, dict):
    fail("has no [package.metadata.package-cli] table naming what the tarball ships")
unknown = sorted(set(table) - {"names", "licenses", "caveats"})
if unknown:
    fail("[package.metadata.package-cli] has %s, which is not names, licenses or caveats"
         % ", ".join(unknown))

names = table.get("names")
if not isinstance(names, dict) or not names:
    fail("[package.metadata.package-cli] names is not a table of dotted name = man section, "
         "e.g. names = { \"mkfs.example\" = 8, \"fs.example\" = 1 }")
word = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")
for name, section in names.items():
    if not word.match(name) or name == repo:
        fail("names lists %r, which is not a tool name" % name)
    if isinstance(section, bool) or not isinstance(section, int) or not 1 <= section <= 9:
        fail("names gives %r the man section %r, not a number from 1 to 9"
             " (a dotted name written unquoted is a nested table)" % (name, section))

licenses = table.get("licenses")
if not isinstance(licenses, list) or not licenses \
        or not all(isinstance(f, str) and word.match(f) for f in licenses):
    fail("[package.metadata.package-cli] licenses is not a list of file names at the root, "
         "e.g. licenses = [\"LICENSE\"]")
if len(set(licenses)) != len(licenses):
    fail("licenses names a file twice")

caveats = table.get("caveats", "packaging/CAVEATS")
if not isinstance(caveats, str) or not caveats or caveats.startswith("/") \
        or ".." in caveats.split("/"):
    fail("caveats is %r, not a path inside the repository" % (caveats,))

print("crate\t" + package["name"])
print("repo\t" + repo)
print("caveats\t" + caveats)
for f in licenses:
    print("licence\t" + f)
for name in sorted(names):
    print("name\t%s\t%d" % (name, names[name]))
' "$CALLER/Cargo.toml" | tr -d '\r')" || exit 1

crate=""
repo=""
caveats=""
licences=()
names=()
sections=()
while IFS=$'\t' read -r key value section; do
    case "$key" in
        crate) crate="$value" ;;
        repo) repo="$value" ;;
        caveats) caveats="$value" ;;
        licence) licences+=("$value") ;;
        name) names+=("$value"); sections+=("$section") ;;
    esac
done <<<"$config"
[ -n "$crate" ] && [ -n "$repo" ] && [ "${#names[@]}" -gt 0 ] && [ "${#licences[@]}" -gt 0 ] \
    || die "could not read what $CALLER/Cargo.toml ships"

# The section of NAME's page: its declared one, and 1 for <repo> itself.
section_of() {
    local i
    [ "$1" != "$repo" ] || { echo 1; return 0; }
    for i in "${!names[@]}"; do
        [ "${names[$i]}" != "$1" ] || { echo "${sections[$i]}"; return 0; }
    done
    return 1
}

# x is what a binary's name ends in on the platform being packaged.
case "$label" in
    windows-*) x=".exe"; tarball="$crate-$version-$label.zip" ;;
    *) x=""; tarball="$crate-$version-$label.tar.gz" ;;
esac

# The zip, written, listed and unpacked by Python's zipfile on every host.
zip_py='
import os, sys, zipfile
verb, archive = sys.argv[1], sys.argv[2]
if verb == "pack":
    root = sys.argv[3]
    members = sorted(
        os.path.relpath(os.path.join(d, f), root).replace(os.sep, "/")
        for d, _, files in os.walk(root) for f in files)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for m in members:
            z.write(os.path.join(root, m), m)
elif verb == "list":
    for n in zipfile.ZipFile(archive).namelist():
        print(n)
elif verb == "unpack":
    zipfile.ZipFile(archive).extractall(sys.argv[3])
'

# ON ANY FAILURE, NO TARBALL: not a partial one, and not one a previous run
# left under the same name, which a caller could otherwise take for this
# run's output. The trap is in place before anything below can fail,
# mktemp included.
work=""
cleanup() {
    local status=$?
    [ -z "$work" ] || rm -rf "$work"
    [ "$status" -eq 0 ] || rm -f "$tarball"
    return "$status"
}
trap cleanup EXIT
rm -f "$tarball"
work="$(mktemp -d)" || die "could not make a working directory"
[ -d "$work" ] || die "could not make a working directory"

# ---- Stage the prefix. ------------------------------------------------------
built="$target_dir/$repo$x"
[ -f "$built" ] && [ -x "$built" ] \
    || die "no built $repo$x at $built (cargo build --release --locked --features cli --bin $repo)"

stage="$work/stage"
mkdir -p "$stage/bin" "$stage/share/$repo"
cp "$built" "$stage/bin/$repo$x"
chmod 755 "$stage/bin/$repo$x"

listed="$("$stage/bin/$repo$x" generate names)" || die "$repo generate names failed"
[ -n "$listed" ] || die "$repo generate names listed no tool names"
# Split on whitespace, never globbed: the names are the binary's output.
set -f
for name in $listed; do
    case "$name" in
        */* | .* | "$repo" | *[!A-Za-z0-9._+-]*)
            die "$repo generate names listed '$name', which is not a tool name" ;;
    esac
done
got_names="$(printf '%s\n' $listed | sort | tr '\n' ' ')"
set +f
want_names="$(printf '%s\n' "${names[@]}" | sort | tr '\n' ' ')"
[ "$got_names" = "$want_names" ] \
    || die "$repo generate names lists [${got_names% }], but Cargo.toml's [package.metadata.package-cli] names [${want_names% }]"
for name in "${names[@]}"; do
    if [ -n "$x" ]; then
        cp "$stage/bin/$repo$x" "$stage/bin/$name$x"
    else
        ln -s "$repo" "$stage/bin/$name"
    fi
done

"$stage/bin/$repo$x" generate man "$stage/share" >/dev/null || die "$repo generate man failed"
"$stage/bin/$repo$x" generate completions "$stage/share" >/dev/null \
    || die "$repo generate completions failed"

source_caveats="$CALLER/$caveats"
[ -s "$source_caveats" ] || die "$caveats is missing or empty"
lines="$(wc -l <"$source_caveats" | tr -d ' ')"
[ "$lines" -le 4 ] || die "$caveats is $lines lines; an installer prints it whole, so at most four"
cp "$source_caveats" "$stage/share/$repo/CAVEATS"
for f in "${licences[@]}"; do
    [ -f "$CALLER/$f" ] && [ -s "$CALLER/$f" ] || die "the licence $f is missing or empty"
    cp "$CALLER/$f" "$stage/$f"
done

# Every staged file has a place in the layout. What the generator wrote is
# checked here rather than trusted: a page in a section no name has, or a
# completion for a name the tarball does not ship, would install as clutter
# nothing owns.
placed() {
    local member="$1" n page section
    case "$member" in
        "bin/$repo$x" | "share/$repo/CAVEATS") return 0 ;;
        bin/*"$x")
            n="${member#bin/}"
            section_of "${n%"$x"}" >/dev/null
            return ;;
        share/zsh/site-functions/_*) section_of "${member#share/zsh/site-functions/_}" >/dev/null; return ;;
        share/bash-completion/completions/*) section_of "${member#share/bash-completion/completions/}" >/dev/null; return ;;
        share/fish/vendor_completions.d/*.fish)
            n="${member#share/fish/vendor_completions.d/}"
            section_of "${n%.fish}" >/dev/null
            return ;;
        share/man/man[1-9]/*)
            section="${member#share/man/man}"
            section="${section%%/*}"
            page="${member##*/}"
            case "$page" in *".$section") page="${page%".$section"}" ;; *) return 1 ;; esac
            for n in "$repo" "${names[@]}"; do
                case "$page" in
                    "$n" | "$n"-*) [ "$(section_of "$n")" = "$section" ] && return 0 ;;
                esac
            done
            return 1 ;;
    esac
    for n in "${licences[@]}"; do
        [ "$member" != "$n" ] || return 0
    done
    return 1
}
staged="$(cd "$stage" && find . \( -type f -o -type l \) | sed 's|^\./||' | sort)"
stray=""
while IFS= read -r member; do
    placed "$member" || stray="$stray $member"
done <<<"$staged"
[ -z "$stray" ] || die "$repo staged files outside the install layout:$stray"

u="$work/unpacked"
mkdir -p "$u"
if [ -n "$x" ]; then
    "$PYTHON" -c "$zip_py" pack "$tarball" "$stage" || die "could not write $tarball"
    # ---- The checks, on what was packed rather than on what was staged. ----
    packed="$("$PYTHON" -c "$zip_py" list "$tarball" | tr -d '\r' | grep -v '/$' | sort)"
    "$PYTHON" -c "$zip_py" unpack "$tarball" "$u" || die "could not unpack $tarball"
    # A zip keeps no mode a Windows user's unpacker would apply; Windows runs
    # a .exe by its name, and this host runs it once it is marked so.
    chmod 755 "$u/bin/"*"$x"
else
    # COPYFILE_DISABLE keeps macOS tar from adding ._ AppleDouble members.
    COPYFILE_DISABLE=1 tar -czf "$tarball" -C "$stage" bin share "${licences[@]}"
    # ---- The checks, on what was packed rather than on what was staged. ----
    # Files and links only: whether a tar lists the directories themselves
    # varies by tar.
    packed="$(tar -tzf "$tarball" | sed 's|^\./||' | grep -v '/$' | sort)"
    tar -xzf "$tarball" -C "$u"
fi
[ "$packed" = "$staged" ] \
    || die "$tarball holds [$(echo $packed)], expected [$(echo $staged)]"

[ -f "$u/bin/$repo$x" ] && [ ! -L "$u/bin/$repo$x" ] \
    || die "bin/$repo$x is not a regular file in $tarball"
[ -x "$u/bin/$repo$x" ] || die "bin/$repo$x is not executable in $tarball"
cmp -s "$u/bin/$repo$x" "$built" || die "bin/$repo$x is not the built $built"
for name in "${names[@]}"; do
    if [ -n "$x" ]; then
        [ -f "$u/bin/$name$x" ] && [ ! -L "$u/bin/$name$x" ] \
            || die "bin/$name$x is not a regular file in $tarball"
        cmp -s "$u/bin/$name$x" "$u/bin/$repo$x" \
            || die "bin/$name$x is not a copy of bin/$repo$x"
    else
        [ -L "$u/bin/$name" ] || die "bin/$name is not a symlink in $tarball"
        target="$(readlink "$u/bin/$name")"
        [ "$target" = "$repo" ] || die "bin/$name points at '$target', not the relative '$repo'"
    fi
done
cmp -s "$u/share/$repo/CAVEATS" "$source_caveats" || die "share/$repo/CAVEATS is not $caveats"
for f in "${licences[@]}"; do
    cmp -s "$u/$f" "$CALLER/$f" || die "$f in $tarball is not the repository's"
done

for name in "$repo" "${names[@]}"; do
    section="$(section_of "$name")"
    for doc in "share/man/man$section/$name.$section" \
               "share/zsh/site-functions/_$name" \
               "share/bash-completion/completions/$name" \
               "share/fish/vendor_completions.d/$name.fish"; do
        [ -s "$u/$doc" ] || die "no $doc for $name in $tarball"
    done
    exe="$u/bin/$name$x"
    "$exe" --help >/dev/null || die "$name --help failed"
    reported="$("$exe" --version)" || die "$name --version failed"
    [ "$reported" = "$name ($crate) $version" ] \
        || die "$name --version says '$reported', expected '$name ($crate) $version'"
done

printf '%s\n' "$tarball"
