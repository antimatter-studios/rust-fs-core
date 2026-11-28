#!/usr/bin/env bash
# `chore prove` runs what the Linux jobs of ci.yml run (#431). CI here does not
# install chore, so CI spells each task's commands out; this is what keeps the
# two from drifting apart. It refuses:
#
#   - a command in a Linux job that no task of `prove` runs, or one `prove`
#     runs without an environment variable the CI step sets;
#   - a job on another platform that `prove` does not list as not covered.
#
# Needs python3 with PyYAML, as test-ci-gate.sh does.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/covers.py" <<'PY'
import re
import shlex
import sys

try:
    import yaml
except ImportError:
    sys.exit("python3 cannot import yaml: install PyYAML (apt-get install python3-yaml, or pip install pyyaml)")

ci_path, chores_path = sys.argv[1:3]
with open(ci_path, encoding="utf-8") as handle:
    jobs = yaml.safe_load(handle)["jobs"]
with open(chores_path, encoding="utf-8") as handle:
    tasks = yaml.safe_load(handle)["tasks"]

ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
# Jobs that judge or classify the others and check nothing in the tree.
NOT_A_CHECK = {"changes", "ci-ok"}


def split(line):
    """(assignments, words) of one command line."""
    words = shlex.split(line, comments=True)
    count = 0
    while count < len(words) and ASSIGNMENT.match(words[count]):
        count += 1
    return dict(word.split("=", 1) for word in words[:count]), words[count:]


def lines(script):
    for line in script.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            yield line


def task_lines(name, seen):
    if name not in tasks:
        sys.exit(f"prove names the task '{name}', which chores.yml does not have")
    if name in seen:
        return
    seen.add(name)
    for cmd in tasks[name].get("cmds", []):
        if isinstance(cmd, dict) and "task" in cmd:
            yield from task_lines(cmd["task"], seen)
        else:
            yield from lines(cmd["cmd"] if isinstance(cmd, dict) else cmd)


prove = shlex.split(" ".join(tasks["prove"]["cmds"]))
if "scripts/prove.sh" not in prove:
    sys.exit("the prove task does not run scripts/prove.sh")
arguments = prove[prove.index("scripts/prove.sh") + 1:]
not_covered, proved = [], []
while arguments:
    word = arguments.pop(0)
    if word == "--not-covered":
        not_covered.append(arguments.pop(0))
    else:
        proved.append(word)

local = [split(line) for name in proved for line in task_lines(name, set())]


def is_setup(line):
    """Preparing the runner: packages, a kernel module, a probe for PyYAML."""
    return all(
        re.match(r"(sudo (apt-get|modprobe) |python3 -c 'import yaml')", part.strip())
        for part in re.split(r"&&|\|\|", line)
    )


def is_shell_frame(line):
    return line in ("set -euo pipefail", "fi") or bool(re.match(r"if \[ [^;&|]* \]; then$", line))


def platform(job):
    needs = job.get("needs", [])
    needs = [needs] if isinstance(needs, str) else needs
    labels = [str(job.get("runs-on", ""))]
    for gate in needs:
        if gate.endswith("-gate"):
            for step in jobs[gate].get("steps", []):
                picked = re.search(r"runner-gate pick (\S+)", step.get("run", ""))
                if picked and gate != "changes-gate":
                    labels.insert(0, picked.group(1))
    for label in labels:
        for prefix, found in (("linux", "linux"), ("ubuntu", "linux"), ("macos", "macos"), ("windows", "windows")):
            if label.startswith(prefix):
                return found
    return None


gaps = []
checked = 0
for job_id, job in jobs.items():
    if job_id.endswith("-gate") or job_id in NOT_A_CHECK:
        continue
    where = platform(job)
    if where is None:
        gaps.append(f"job {job_id}: cannot tell which platform it runs on")
        continue
    if where != "linux":
        if job.get("name", job_id) not in not_covered:
            gaps.append(f"job {job_id}: runs on {where}, and prove does not list '{job.get('name', job_id)}' as --not-covered")
        continue
    for step in job.get("steps", []):
        env = {key: str(value) for key, value in step.get("env", {}).items()}
        for line in lines(step.get("run", "")):
            if is_setup(line) or is_shell_frame(line):
                continue
            checked += 1
            _, words = split(line)
            # transfer-floors.sh TIER runs the generic floors and the host's.
            floors = re.match(r"bash scripts/core\.sh test-floor --targets \.github/transfer(-linux|-macos|-windows)?-test-floors\.txt (\S+)$", line)
            if floors:
                words = ["bash", "scripts/transfer-floors.sh", floors.group(2)]
            matches = [assigned for assigned, ran in local if ran == words]
            if not matches:
                gaps.append(f"job {job_id}: no task of prove runs: {line}")
            elif "cargo" in words:
                if not any(all(assigned.get(k) == v for k, v in env.items()) for assigned in matches):
                    gaps.append(f"job {job_id}: prove runs this without the CI step's environment {env}: {line}")

if not checked:
    gaps.append("no Linux command was found in ci.yml, so nothing was compared")
if gaps:
    print("\n".join(gaps))
    sys.exit(1)
print(f"prove covers the {checked} Linux CI commands")
PY

covers() { python3 "$TMP/covers.py" "$1" "$2"; }
CI="$ROOT/.github/workflows/ci.yml"
CHORES="$ROOT/chores.yml"

covers "$CI" "$CHORES"

# The guard can fail: each mutation below is a way the two drift apart.
refuses() {
    local what="$1" expected="$2" ci="$3" chores="$4" output
    if output="$(covers "$ci" "$chores" 2>&1)"; then
        echo "the guard accepted $what"; exit 1
    fi
    case "$output" in
        *"$expected"*) ;;
        *) echo "the guard refused $what for another reason: $output"; exit 1 ;;
    esac
}
mutate() {
    python3 - "$1" "$2" "$3" "$4" <<'PY'
import sys
source, target, old, new = sys.argv[1:]
text = open(source, encoding="utf-8").read()
if old not in text:
    sys.exit(f"the mutation found no {old!r} in {source}")
open(target, "w", encoding="utf-8").write(text.replace(old, new, 1))
PY
}

{
    cat "$CI"
    printf '\n  extra:\n    runs-on: ubuntu-latest\n    steps:\n      - run: cargo run --example extra\n'
} > "$TMP/new-job.yml"
refuses "a new Linux job" "job extra: no task of prove runs: cargo run --example extra" "$TMP/new-job.yml" "$CHORES"
{
    cat "$CI"
    printf '\n  extra:\n    name: extra on a Mac\n    runs-on: macos-latest\n    steps:\n      - run: true\n'
} > "$TMP/mac-job.yml"
refuses "an unlisted macOS job" "does not list 'extra on a Mac' as --not-covered" "$TMP/mac-job.yml" "$CHORES"
mutate "$CHORES" "$TMP/dropped.yml" " check:msrv " " "
refuses "a dropped task" "job msrv: no task of prove runs" "$CI" "$TMP/dropped.yml"
mutate "$CHORES" "$TMP/no-env.yml" 'RUSTFLAGS="-C link-dead-code" ' ""
refuses "a lost environment variable" "without the CI step's environment" "$CI" "$TMP/no-env.yml"
echo "the guard refuses each way prove and ci.yml drift apart"
