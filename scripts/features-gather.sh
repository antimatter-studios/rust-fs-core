#!/usr/bin/env bash
# Gather features.d/ into docs/features.md, for a release.
#
# A repository's features page (docs/features.md) is one table per `## `
# section, a row per feature, and nearly every pull request changes a row of
# it. Two pull requests that both edit the page conflict in the merge queue as
# surely as two that both edit the top of CHANGELOG.md, so a pull request does
# not edit the page: it adds one fragment under features.d/, named for its
# branch, and a release commit runs this to apply every fragment to the page
# and delete them -- the rows move rather than copy, as changelog-gather.sh
# moves changelog entries.
#
# A fragment is one or more `## Section` headings the page already has, each
# over lines of three kinds:
#
#   | Feature | State | Since | Tracking | Checked by |   a row, in that
#       section's column layout. Its first cell is its key: a row whose key
#       the section already has replaces that row where it stands; a new key
#       is appended to the end of the section's table.
#   remove: <key>      deletes the row with exactly that key.
#   rename: <key>      the row on the next line replaces the row with exactly
#       that key, where it stands, under the new row's own key.
#
# The release then turns every `Unreleased` in a Since cell into VERSION --
# in the rows it wrote, and in any the page still held -- so `Unreleased (#N)`
# ships as `VERSION (#N)`.
#
# It refuses, changing nothing, when anything does not apply: a section the
# page does not have (or has twice, or with no table, or with more than one),
# a row with the wrong number of cells, a remove or rename of a key the
# section does not hold exactly once, a rename onto a key it already holds,
# and two fragments (or one, twice) writing the same key of the same section
# -- naming both files. Everything is checked before anything is written, so
# a refusal never leaves a half-applied page. Fragments are applied in
# file-name order, so the release's page comes out the same on every machine.
#
# features-check.sh, which `chore lint` runs, is this with --check: the same
# parser and the same rules, writing nothing, plus one more -- the page holds
# no Unreleased of its own, because an unreleased row is a fragment.
#
# Python 3 (standard library only) reads the tables: splitting a row into
# cells and holding every edit until all of them are known to apply is what
# awk does differently on every runner. ci-gate.sh already needs python3.
#
# Usage: features-gather.sh VERSION   apply the fragments as release VERSION
#        features-gather.sh --check   check them, writing nothing
#   VERSION  e.g. 0.3.10 (a leading v is dropped)
#   FS_CORE_CALLER names the repository when run in place from rust-fs-core.
set -euo pipefail

mode=gather
version=""
if [ "${1:-}" = "--check" ]; then
    mode=check
    tool=features-check
    [ $# -eq 1 ] || { echo "usage: features-check.sh" >&2; exit 2; }
else
    tool=features-gather
    [ $# -eq 1 ] || { echo "usage: features-gather.sh VERSION" >&2; exit 2; }
    version="${1#v}"
    case "$version" in
        "" | *[!0-9A-Za-z.+-]*) echo "features-gather: '$1' is not a version" >&2; exit 2 ;;
    esac
fi

ROOT="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
DIR="$ROOT/features.d"

command -v python3 >/dev/null || { echo "$tool: python3 is required" >&2; exit 2; }

if ! python3 - "$mode" "$ROOT" "$version" <<'PY'
import os
import re
import sys

mode, root, version = sys.argv[1], sys.argv[2], sys.argv[3]
tool = "features-check" if mode == "check" else "features-gather"
page_rel = "docs/features.md"
page_path = os.path.join(root, page_rel)
frag_dir = os.path.join(root, "features.d")
UNRELEASED = re.compile(r"\bUnreleased\b")
MARKER = re.compile(r"^(remove|rename):(.*)$")

problems = []


def problem(message):
    problems.append(message)


def split_row(line):
    """The cells of a table row, trimmed, or None when the line is not one.
    `\\|` is a pipe inside a cell, as GitHub reads it."""
    s = line.strip()
    if not s.startswith("|"):
        return None
    cells, cur, i = [], "", 1
    while i < len(s):
        if s[i] == "\\" and s[i + 1 : i + 2] == "|":
            cur += "\\|"
            i += 2
            continue
        if s[i] == "|":
            cells.append(cur.strip())
            cur = ""
        else:
            cur += s[i]
        i += 1
    if cur.strip():
        cells.append(cur.strip())
    return cells


def is_separator(cells):
    return bool(cells) and all(re.fullmatch(r":?-+:?", c) for c in cells)


def render(cells):
    return "|" + "|".join(" %s " % c if c else " " for c in cells) + "|"


class Table:
    def __init__(self, header, rows, end):
        self.header = header  # the header's cells
        self.rows = rows  # line indexes of the body rows
        self.end = end  # the index of the first line after the table
        self.since = next(
            (i for i, c in enumerate(header) if c.lower() == "since"), None
        )


class Section:
    def __init__(self, name, line):
        self.name, self.line, self.tables = name, line, []


# --- The fragments: every file under features.d/ but its README. ----------
fragments = []
if os.path.isdir(frag_dir):
    for name in sorted(os.listdir(frag_dir)):
        path = os.path.join(frag_dir, name)
        if name in ("README.md", ".gitkeep"):
            continue
        if not name.endswith(".md"):
            problem("features.d/%s is not a fragment: only *.md fragments and README.md belong there" % name)
        elif not os.path.isfile(path):
            problem("features.d/%s is not a file" % name)
        else:
            fragments.append(name)

if not os.path.isfile(page_path):
    if fragments:
        problem("features.d/ holds %d fragment(s) but there is no %s to apply them to" % (len(fragments), page_rel))
    for p in problems:
        print("%s: %s" % (tool, p), file=sys.stderr)
    if problems:
        sys.exit(1)
    print("%s: no %s, nothing to %s" % (tool, page_rel, "check" if mode == "check" else "gather"))
    sys.exit(0)

if mode == "check" and not os.path.isdir(frag_dir):
    problem("there is no features.d/: a pull request records its rows there, one fragment per pull request, and a release applies them to %s" % page_rel)

# --- The page: its `## ` sections and the tables under them. --------------
with open(page_path, encoding="utf-8") as f:
    lines = f.read().split("\n")

sections, tables, section, fence, i = [], [], None, False, 0
while i < len(lines):
    line = lines[i]
    if line.lstrip().startswith(("```", "~~~")):
        fence = not fence
    elif fence:
        pass
    elif line.startswith("## "):
        section = Section(line[3:].strip(), i)
        sections.append(section)
    elif line.startswith("# "):
        section = None
    else:
        header = split_row(line)
        below = split_row(lines[i + 1]) if i + 1 < len(lines) else None
        if header and below and is_separator(below):
            j = i + 2
            while j < len(lines) and split_row(lines[j]) is not None:
                j += 1
            table = Table(header, list(range(i + 2, j)), j)
            tables.append(table)
            if section is not None:
                section.tables.append(table)
            i = j
            continue
    i += 1

by_name = {}
for s in sections:
    by_name.setdefault(s.name, []).append(s)


def keys_of(table):
    keys = {}
    for idx in table.rows:
        cells = split_row(lines[idx])
        keys.setdefault(cells[0] if cells else "", []).append(idx)
    return keys


# --- Parse every fragment, then hold each edit against the page. ----------
class Edit:
    def __init__(self, kind, frag, line, section, key, cells=None, old=None):
        self.kind, self.frag, self.line = kind, frag, line
        self.section, self.key, self.cells, self.old = section, key, cells, old


edits = []
for name in fragments:
    where = "features.d/" + name
    with open(os.path.join(frag_dir, name), encoding="utf-8") as f:
        flines = f.read().split("\n")
    heading, counted, rename, headings = None, 0, None, 0

    def close_heading():
        if heading is not None and counted == 0:
            problem("%s: the section \"%s\" has no rows" % (where, heading))

    for n, raw in enumerate(flines, 1):
        s = raw.strip()
        if not s:
            continue
        if rename is not None and not s.startswith("|"):
            problem("%s:%d: `rename: %s` is not followed by the row it becomes" % (where, rename[1], rename[0]))
            rename = None
        if raw.startswith("## ") and raw[3:].strip():
            close_heading()
            heading, counted, headings = raw[3:].strip(), 0, headings + 1
            continue
        if s.startswith("#"):
            problem("%s:%d: only `## Section` headings belong in a fragment, naming a section of %s" % (where, n, page_rel))
            continue
        if heading is None:
            problem("%s:%d: text before the first `## Section` heading" % (where, n))
            continue
        marker = MARKER.match(s)
        if marker:
            key = marker.group(2).strip()
            if not key:
                problem("%s:%d: `%s:` names no key" % (where, n, marker.group(1)))
            elif marker.group(1) == "remove":
                edits.append(Edit("remove", name, n, heading, key))
                counted += 1
            else:
                rename = (key, n)
            continue
        cells = split_row(s)
        if cells is None:
            problem("%s:%d: not a table row, a `remove:` or `rename:` line, or a `## Section` heading" % (where, n))
            continue
        if is_separator(cells):
            problem("%s:%d: a separator row: a fragment holds rows only, and the page supplies the header" % (where, n))
            continue
        if not cells or not cells[0]:
            problem("%s:%d: the first cell is empty, and the first cell is the row's key" % (where, n))
            continue
        edits.append(Edit("row", name, n, heading, cells[0], cells, rename[0] if rename else None))
        rename = None
        counted += 1
    if rename is not None:
        problem("%s:%d: `rename: %s` is not followed by the row it becomes" % (where, rename[1], rename[0]))
    close_heading()
    if headings == 0 and not any(p.startswith(where + ":") for p in problems):
        problem("%s: no `## Section` heading and no rows: the fragment is empty" % where)

written = {}
plans = {}  # id(table) -> {"replace": {line index: cells}, "delete": {line index}, "append": [cells]}
for e in edits:
    where = "features.d/%s:%d" % (e.frag, e.line)
    found = by_name.get(e.section, [])
    if not found:
        problem("%s: %s has no section `## %s`" % (where, page_rel, e.section))
        continue
    if len(found) > 1:
        problem("%s: %s has %d sections named `## %s`, so the fragment cannot say which" % (where, page_rel, len(found), e.section))
        continue
    sec = found[0]
    if len(sec.tables) != 1:
        problem("%s: `## %s` in %s holds %s, and a fragment writes into exactly one" % (where, e.section, page_rel, "no table" if not sec.tables else "%d tables" % len(sec.tables)))
        continue
    table = sec.tables[0]
    if e.kind == "row":
        if len(e.cells) != len(table.header):
            problem("%s: the row has %d cell(s), and `## %s` has %d columns (%s)" % (where, len(e.cells), e.section, len(table.header), " | ".join(table.header)))
            continue
        if e.cells == table.header:
            problem("%s: the header row: a fragment holds rows only, and the page supplies the header" % where)
            continue
    for key in [e.key] + ([e.old] if e.old is not None else []):
        first = written.get((e.section, key))
        if first:
            problem("features.d/%s:%d and %s both write `%s` in `## %s`: a row is written by one fragment, so edit the one that writes it already" % (first[0], first[1], where, key, e.section))
        else:
            written[(e.section, key)] = (e.frag, e.line)
    keys = keys_of(table)
    plan = plans.setdefault(id(table), {"replace": {}, "delete": set(), "append": []})
    target = e.old if e.old is not None else e.key
    rows = keys.get(target, [])
    if e.kind == "remove" or e.old is not None:
        verb = "remove" if e.kind == "remove" else "rename"
        if not rows:
            problem("%s: there is no row `%s` in `## %s` to %s" % (where, target, e.section, verb))
            continue
        if len(rows) > 1:
            problem("%s: `## %s` has %d rows keyed `%s`, so there is no telling which to %s" % (where, e.section, len(rows), target, verb))
            continue
        if e.kind == "remove":
            plan["delete"].add(rows[0])
            continue
        if e.key == e.old:
            problem("%s: `rename: %s` keeps the same key: write the row without `rename:`" % (where, e.old))
            continue
        if e.key in keys:
            problem("%s: `## %s` already has a row `%s` to rename onto: remove it, or rename to a key the section does not hold" % (where, e.section, e.key))
            continue
        plan["replace"][rows[0]] = e.cells
    elif len(rows) > 1:
        problem("%s: `## %s` has %d rows keyed `%s`, so there is no telling which to replace" % (where, e.section, len(rows), e.key))
    elif rows:
        plan["replace"][rows[0]] = e.cells
    else:
        plan["append"].append(e.cells)


def unreleased(table, cells):
    return table.since is not None and len(cells) > table.since and UNRELEASED.search(cells[table.since])


if mode == "check":
    for table in tables:
        for idx in table.rows:
            cells = split_row(lines[idx])
            if unreleased(table, cells):
                problem("%s:%d: `%s` says Unreleased under %s: an unreleased row is a fragment in features.d/, and the page takes it only when a release gathers it" % (page_rel, idx + 1, cells[0], table.header[table.since]))

if problems:
    for p in problems:
        print("%s: %s" % (tool, p), file=sys.stderr)
    print("%s: %d problem(s)%s" % (tool, len(problems), "" if mode == "check" else "; nothing was changed"), file=sys.stderr)
    sys.exit(1)

if mode == "check":
    print("%s: %d fragment(s) in features.d/, each applies to %s, and the page holds no Unreleased row of its own" % (tool, len(fragments), page_rel))
    sys.exit(0)

# --- Write: every edit is known to apply. ---------------------------------
shipped = [0]


def emit(table, cells, original=None):
    if unreleased(table, cells):
        cells = list(cells)
        cells[table.since] = UNRELEASED.sub(version, cells[table.since])
        shipped[0] += 1
        return render(cells)
    return original if original is not None else render(cells)


owner = {idx: t for t in tables for idx in t.rows}
ends = {t.end: t for t in tables}
out = []
for idx in range(len(lines) + 1):
    table = ends.get(idx)
    if table is not None:
        for cells in plans.get(id(table), {}).get("append", []):
            out.append(emit(table, cells))
    if idx == len(lines):
        break
    table = owner.get(idx)
    if table is None:
        out.append(lines[idx])
        continue
    plan = plans.get(id(table), {"replace": {}, "delete": set()})
    if idx in plan["delete"]:
        continue
    if idx in plan["replace"]:
        out.append(emit(table, plan["replace"][idx]))
    else:
        out.append(emit(table, split_row(lines[idx]), lines[idx]))

with open(page_path, "w", encoding="utf-8") as f:
    f.write("\n".join(out))

print("%s: %d fragment(s) gathered into %s as %s; %d Since cell(s) moved from Unreleased to %s" % (tool, len(fragments), page_rel, version, shipped[0], version))
PY
then
    exit 1
fi

[ "$mode" = gather ] || exit 0

# The fragments move: tracked ones leave the index, so the release commit
# deletes them; untracked ones are simply removed.
[ -d "$DIR" ] || exit 0
while IFS= read -r f; do
    if git -C "$ROOT" ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
        git -C "$ROOT" rm --quiet -- "$f"
    else
        rm -f -- "$f"
    fi
done < <(find "$DIR" -maxdepth 1 -type f -name '*.md' ! -name README.md | LC_ALL=C sort)
