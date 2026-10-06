#!/usr/bin/env bash
# ci-gate.sh -- the one required check stands for every job. Run it as
# `scripts/core.sh ci-gate` from any repository in the family; this file is
# the only copy. It reads the CALLER's ci.yml and .github-guard
# ($FS_CORE_CALLER, which scripts/core.sh sets; run directly, the repository
# this file is in).
#
# Two halves, and both must hold or the aggregate is decoration:
#
#   1. ci.yml    — the aggregate job `needs:` every other gating job, carries
#                  `if: always()`, names no job that does not exist, and its
#                  steps read every result it needs: `toJSON(needs)` or
#                  `needs.*.result` as a whole, or each `needs.<job>.result`.
#                  What a script handed `toJSON(needs)` does with it is that
#                  script's business; the gate sees only that it was handed.
#   2. .github-guard — requires that aggregate and nothing else.
#
# THE OVERRIDES, all optional and read from the environment. Paths are relative
# to the caller's root:
#   CI_GATE_WORKFLOW     the gating workflow   (.github/workflows/ci.yml)
#   CI_GATE_GUARD        the declaration       (.github-guard)
#   CI_GATE_AGGREGATE    the aggregate job     (ci-ok)
#   CI_GATE_NON_GATING   space-separated jobs that carry `if:` or
#                        `continue-on-error:` and are deliberately advisory
#
# WHY THIS IS A SCRIPT AND NOT A TEST. It parses a YAML file and compares
# strings; it exercises nothing a crate ships. As a `cargo test` it also
# counted towards the executed-test floor the gate itself enforces, so a repo
# could satisfy its floor partly by checking its own CI config.
#
# WHY IT IS NOT LINE-SCANNED. A quoted key, a flow mapping, and a `run: |` block
# whose CONTENTS look like a job key are all ordinary YAML that a line scan
# reads wrongly -- and a guard that misreads its input reports protection it is
# not providing. YAML 1.2 semantics matter too: GitHub's `on:` key must stay the
# string `on` rather than folding into a boolean, since that is the key telling
# a gating workflow from a release one.
#
# THE GUARD IS READ THE WAY github-guard READS IT: `git config --file GUARD
# --no-includes --get-all checks.required`, one check per `required =` line,
# each value trimmed and kept whole. A check name can hold spaces -- `full test
# suite (fixtures + integration)` is one -- so splitting a value on whitespace
# reports checks that do not exist; and git drops a trailing `; comment` that a
# hand-rolled parser reads as more names. A guard git cannot parse fails.
set -uo pipefail

CI_GATE_API_VERSION=1
if [ "${1:-}" = "--version" ]; then
    printf 'rust-fs-core-ci-gate %s\n' "$CI_GATE_API_VERSION"
    exit 0
fi
[ $# -eq 0 ] || { echo "usage: ci-gate.sh   (configured by CI_GATE_* in the environment)" >&2; exit 2; }

ROOT="${FS_CORE_CALLER:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
WORKFLOW="${CI_GATE_WORKFLOW:-.github/workflows/ci.yml}"
AGGREGATE="${CI_GATE_AGGREGATE:-ci-ok}"
GUARD="${CI_GATE_GUARD:-.github-guard}"
# Jobs exempt from gating: they carry `if:` or `continue-on-error:` and are
# deliberately advisory. Space separated. An exemption for a job that does not
# exist is an exemption waiting to silently cover a future job of that name.
NON_GATING="${CI_GATE_NON_GATING:-}"

cd "$ROOT" || exit 1
command -v python3 >/dev/null || { echo "ci-gate: python3 is required" >&2; exit 2; }
command -v git >/dev/null || { echo "ci-gate: git is required to read $GUARD" >&2; exit 2; }

python3 - "$WORKFLOW" "$AGGREGATE" "$GUARD" "$NON_GATING" <<'PY'
import sys, os, re, subprocess
wf, agg, guard, non_gating = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4].split()
fails = []

try:
    import yaml
except ImportError:
    print("ci-gate: python3 yaml module is required (pip install pyyaml)", file=sys.stderr)
    sys.exit(2)

if not os.path.exists(wf):
    print(f"ci-gate: {wf} is missing", file=sys.stderr); sys.exit(1)

# YAML 1.1 folds `on:` to True. Re-key it back so the trigger is readable.
doc = yaml.safe_load(open(wf)) or {}
triggers = doc.get("on", doc.get(True, {})) or {}
if isinstance(triggers, str):
    triggers = {triggers: None}
if isinstance(triggers, list):
    triggers = {t: None for t in triggers}

jobs = doc.get("jobs") or {}

if "pull_request" not in triggers:
    fails.append(
        f"`{wf}` does not run on `pull_request`, so nothing in it can gate a merge. "
        f"A required check that never reports reads to GitHub as permanently pending, "
        f"not failing. Triggers: {sorted(triggers)}")

if agg not in jobs:
    fails.append(
        f"`{wf}` has no `{agg}` job, so protection has to name every job by hand -- "
        f"and that list drifts the moment one is renamed, split or added. "
        f"Jobs present: {sorted(jobs)}")
else:
    body = jobs[agg] or {}
    needs = body.get("needs") or []
    if isinstance(needs, str):
        needs = [needs]

    cond = str(body.get("if", "")).strip()
    norm = cond.replace("${{", "").replace("}}", "").strip()
    if norm != "always()":
        if not cond:
            fails.append(
                f"`{agg}` does not carry `if: always()`, so a cancelled or skipped job "
                f"leaves it skipped too -- and a skipped required check never reports. "
                f"A job that was skipped is not a job that passed.")
        else:
            fails.append(
                f"`{agg}` carries `if: {cond}` rather than `if: always()`. Any condition "
                f"that can be false is a condition under which the one required check "
                f"does not report, and a required check that does not report reads as "
                f"permanently pending.")

    for n in needs:
        if n not in jobs:
            fails.append(f"`{agg}` needs `{n}`, which is not a job in `{wf}`.")

    # The aggregate must READ what it needs (#198). A job in `needs:` whose
    # result no step looks at gates nothing: it can fail and the aggregate
    # still passes. Accepted: the whole context -- `toJSON(needs)` (GitHub's
    # function names are case-insensitive) or `needs.*.result` -- or each
    # need's own `needs.<job>.result` / `needs['<job>'].result`. Read from
    # every string in the steps, so `run:`, `env:` and `if:` all count.
    def strings(node):
        if isinstance(node, str):
            yield node
        elif isinstance(node, dict):
            for k, v in node.items():
                yield from strings(k)
                yield from strings(v)
        elif isinstance(node, list):
            for v in node:
                yield from strings(v)
    text = "\n".join(strings(body.get("steps") or []))
    whole = re.search(r"(?i)\btojson\(\s*needs\s*\)", text) or \
        re.search(r"\bneeds\.\*\.result\b", text)
    if not whole:
        for n in needs:
            spelled = re.escape(n)
            if not re.search(r"\bneeds(\." + spelled + r"|\[\s*['\"]" + spelled +
                             r"['\"]\s*\])\.result\b", text):
                fails.append(
                    f"`{agg}` needs `{n}` but no step of it reads `needs.{n}.result`, so "
                    f"that job gates nothing: it can fail and `{agg}` still passes. Judge "
                    f"`toJSON(needs)` as a whole, so a job added to `needs:` is judged "
                    f"without editing the step.")

    for name in non_gating:
        if name not in jobs:
            fails.append(
                f"`{name}` is declared non-gating but is not a job in `{wf}`. An exemption "
                f"for a job that does not exist is an exemption waiting to silently cover "
                f"a future job of that name.")
            continue
        jb = jobs[name] or {}
        if "if" not in jb and "continue-on-error" not in jb:
            fails.append(
                f"`{name}` is declared non-gating but carries neither `if:` nor "
                f"`continue-on-error:`. It runs unconditionally and its failure is a real "
                f"failure, so exempting it takes a working gate off a working job.")

    for name, jb in jobs.items():
        if name == agg:
            continue
        jb = jb or {}
        conditional = ("if" in jb) or ("continue-on-error" in jb)
        declared = name in non_gating
        in_needs = name in needs
        if not conditional and not declared and not in_needs:
            fails.append(
                f"`{agg}` does not need `{name}`, so that job gates nothing: it can go red "
                f"and the merge still goes through. `{agg}` needs {needs}")
        if conditional and not declared and in_needs:
            fails.append(
                f"`{name}` carries a job-level `if:`/`continue-on-error:` and is in "
                f"`{agg}`'s needs without being declared non-gating. Decide which it is: "
                f"a skipped dependency is judged by `always()`, so an undeclared "
                f"conditional job silently changes what the gate means.")

# .github-guard: git-config, read by git, as github-guard reads it. Each
# `required =` line is one check, whatever it contains.
required = []
if not os.path.exists(guard):
    fails.append(f"`{guard}` is missing, so nothing declares what protection should require.")
else:
    valid = subprocess.run(["git", "config", "--file", guard, "--no-includes", "--list"],
                           capture_output=True, text=True)
    if valid.returncode != 0:
        fails.append(
            f"`{guard}` is not valid git-config, so github-guard ignores it and protection "
            f"falls back to discovering checks: {valid.stderr.strip()}")
    else:
        got = subprocess.run(["git", "config", "--file", guard, "--no-includes", "--get-all",
                              "checks.required"], capture_output=True, text=True)
        required = [v.strip() for v in got.stdout.split("\n") if v.strip()]

if required != [agg]:
    fails.append(
        f"`{guard}` requires {required}; it should name `{agg}` alone. Requiring the "
        f"aggregate rather than each job means a job can be renamed, split or made "
        f"conditional without leaving a required check that never reports.")

if fails:
    print("ci-gate: the one required check does not stand for every job", file=sys.stderr)
    for f in fails:
        print(f"  - {f}", file=sys.stderr)
    sys.exit(1)

print(f"ci-gate: {guard} requires `{agg}`, and `{agg}` needs every job in {wf}")
PY
