#!/usr/bin/env bash
# ci-gate holds a CALLER's ci.yml and .github-guard, the way every repository
# in the family runs it: `bash scripts/core.sh ci-gate` from its own tree.
#
# Each case builds a caller tree under this repository's tmp/ with a copy of
# scripts/core.sh, points FS_CORE_ROOT here, and asserts on what the gate says.
# A gate that cannot fail is indistinguishable from no gate, so every refusal
# it exists for is proved against a tree that earns it, and the passing tree
# shares nothing with this repository's own ci.yml -- a gate that read core's
# files instead of the caller's would pass this one tree and fail the others.
#
# THE GUARD IS READ AS GIT READS IT. github-guard reads `.github-guard` with
# `git config --get-all checks.required`, one check per `required =` line, and
# a check name can hold spaces: `full test suite (fixtures + integration)`.
# The gate used to split each value on whitespace and commas, so it reported
# checks that do not exist and refused a trailing `; comment` git ignores.
#
# This is a shell test rather than a cargo test because the gate needs
# python3's yaml module, which the ubuntu job running these provides and the
# macOS and Windows legs of `test` do not promise.
#
#   bash tests/scripts/test-ci-gate.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

command -v python3 >/dev/null 2>&1 || { echo "FAIL  python3 is required" >&2; exit 1; }
python3 -c 'import yaml' 2>/dev/null ||
    { echo "FAIL  the python3 yaml module is required (pip install pyyaml)" >&2; exit 1; }
command -v git >/dev/null 2>&1 || { echo "FAIL  git is required" >&2; exit 1; }

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/ci-gate.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

# caller NAME GUARD-BODY: a repository with its own ci.yml (read from stdin)
# and .github-guard, carrying core's bootstrap.
caller() {
    local dir="$SANDBOX/$1"
    mkdir -p "$dir/scripts" "$dir/.github/workflows"
    cp "$REPO/scripts/core.sh" "$dir/scripts/core.sh"
    cat > "$dir/.github/workflows/ci.yml"
    printf '%s' "$2" > "$dir/.github-guard"
    echo "$dir"
}

# gate DIR [ENV...]: run the gate from DIR; stdout+stderr in $said, status in $status.
gate() {
    local dir="$1"; shift
    said="$(cd "$dir" && env FS_CORE_ROOT="$REPO" "$@" bash scripts/core.sh ci-gate 2>&1)"
    status=$?
}

WORKFLOW='on:
  pull_request:
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: |
          echo "lint:"
  lint:
    runs-on: ubuntu-latest
    steps: [{run: "true"}]
  ci-ok:
    if: always()
    needs: [build, lint]
    runs-on: ubuntu-latest
    steps:
      - env:
          NEEDS: ${{ toJSON(needs) }}
        run: echo "$NEEDS" | jq -e '"'"'all(.[]; .result == "success")'"'"'
'
ONE='[checks]
	required = ci-ok
'

# --- 1. A caller that holds both halves passes, judged on ITS files. -------
dir="$(caller clean "$ONE" <<<"$WORKFLOW")"
gate "$dir"
if [ "$status" -eq 0 ] && grep -qF 'requires `ci-ok`, and `ci-ok` needs every job' <<<"$said"; then
    ok "a caller whose ci-ok needs every job and whose guard names ci-ok alone passes"
else
    fail "a clean caller was refused (status $status):"$'\n'"$said"
fi

# --- 2. A job ci-ok does not need is named. --------------------------------
dir="$(caller unneeded "$ONE" <<<"${WORKFLOW/needs: \[build, lint\]/needs: [build]}")"
gate "$dir"
if [ "$status" -eq 1 ] && grep -qF '`ci-ok` does not need `lint`' <<<"$said"; then
    ok "a job missing from ci-ok's needs fails, by name"
else
    fail "a job outside ci-ok's needs was not named (status $status):"$'\n'"$said"
fi

# --- 3. No aggregate at all, the shape go-networkfs had. -------------------
dir="$(caller no-aggregate "$ONE" <<<"${WORKFLOW%%  ci-ok:*}")"
gate "$dir"
if [ "$status" -eq 1 ] && grep -qF 'has no `ci-ok` job' <<<"$said"; then
    ok "a workflow with no ci-ok job fails"
else
    fail "a workflow with no aggregate was accepted (status $status):"$'\n'"$said"
fi

# --- 4. A check name with spaces is one check, named whole. ---------------
dir="$(caller spaced '[checks]
	required = ci-ok
	required = full test suite (fixtures + integration)
	required = "validate format (Windows chkdsk)"
' <<<"$WORKFLOW")"
gate "$dir"
want="requires ['ci-ok', 'full test suite (fixtures + integration)', 'validate format (Windows chkdsk)']"
if [ "$status" -eq 1 ] && grep -qF "$want" <<<"$said"; then
    ok "a guard naming checks with spaces is reported one check per required line"
else
    fail "the guard's checks were not named as written (status $status), wanted $want:"$'\n'"$said"
fi
if grep -qF "'(fixtures'" <<<"$said"; then
    fail "the guard's check names were split on whitespace:"$'\n'"$said"
fi

# --- 5. What git ignores, the gate ignores: a trailing comment. -----------
dir="$(caller commented '[checks]
	required = ci-ok   ; the aggregate, and nothing else
' <<<"$WORKFLOW")"
gate "$dir"
if [ "$status" -eq 0 ]; then
    ok "a trailing git-config comment is not read as more check names"
else
    fail "a trailing comment git ignores was read as checks (status $status):"$'\n'"$said"
fi

# --- 6. A required line outside [checks] is not a required check. ---------
dir="$(caller elsewhere '[checks]
	required = ci-ok
[merge]
	required = something-else
' <<<"$WORKFLOW")"
gate "$dir"
if [ "$status" -eq 0 ]; then
    ok "only checks.required is read, as github-guard reads it"
else
    fail "a key outside [checks] was read as a required check (status $status):"$'\n'"$said"
fi

# --- 7. A guard git cannot parse fails, rather than reading as empty. -----
dir="$(caller unparsable '[checks
	required = ci-ok
' <<<"$WORKFLOW")"
gate "$dir"
if [ "$status" -eq 1 ] && grep -qF 'is not valid git-config' <<<"$said"; then
    ok "a guard git cannot parse fails"
else
    fail "an unparsable guard was not refused as such (status $status):"$'\n'"$said"
fi

# --- 8. ci-ok must READ what it needs, not only need it (#198). ------------
#
# A job in `needs:` whose result the step never looks at gates nothing: it
# can fail and ci-ok still passes. Three repositories in the family had
# exactly this, a hand-kept list in the step that left out `semver`.
listed='  ci-ok:
    if: always()
    needs: [build, lint]
    runs-on: ubuntu-latest
    steps:
      - name: every gating job must have concluded success
        run: |
          for one in "build:${{ needs.build.result }}"; do
            [ "${one#*:}" = success ] || exit 1
          done
'
dir="$(caller unread "$ONE" <<<"${WORKFLOW%%  ci-ok:*}$listed")"
gate "$dir"
if [ "$status" -eq 1 ] && grep -qF '`ci-ok` needs `lint` but no step of it reads `needs.lint.result`' <<<"$said"; then
    ok "a need the aggregate's steps never read fails, by name"
else
    fail "a need ci-ok never reads was accepted (status $status):"$'\n'"$said"
fi
if grep -qF 'needs.build.result' <<<"$said"; then
    fail "a need the step does read was reported as unread:"$'\n'"$said"
fi

# Every need named, in each spelling GitHub accepts, passes.
named='  ci-ok:
    if: always()
    needs: [build, lint]
    runs-on: ubuntu-latest
    steps:
      - if: ${{ needs.build.result != '"'"'success'"'"' || needs['"'"'lint'"'"'].result != '"'"'success'"'"' }}
        run: exit 1
'
dir="$(caller named "$ONE" <<<"${WORKFLOW%%  ci-ok:*}$named")"
gate "$dir"
if [ "$status" -eq 0 ]; then
    ok "a step naming every need's result, dotted or indexed, passes"
else
    fail "a step that reads every need was refused (status $status):"$'\n'"$said"
fi

# The whole context, in either of GitHub's spellings, passes; so does needs.*.
for whole in 'toJson(needs)' 'contains(needs.*.result, '"'"'failure'"'"')'; do
    body="  ci-ok:
    if: always()
    needs: [build, lint]
    runs-on: ubuntu-latest
    steps:
      - env:
          VERDICT: \${{ $whole }}
        run: test -n \"\$VERDICT\"
"
    dir="$(caller "whole-${whole%%(*}" "$ONE" <<<"${WORKFLOW%%  ci-ok:*}$body")"
    gate "$dir"
    if [ "$status" -eq 0 ]; then
        ok "a step reading the whole needs context ($whole) passes"
    else
        fail "a step reading the whole needs context ($whole) was refused (status $status):"$'\n'"$said"
    fi
done

# A step with no steps at all, or none that touch needs, reads nothing.
blind='  ci-ok:
    if: always()
    needs: [build, lint]
    runs-on: ubuntu-latest
    steps: [{run: "true"}]
'
dir="$(caller blind "$ONE" <<<"${WORKFLOW%%  ci-ok:*}$blind")"
gate "$dir"
if [ "$status" -eq 1 ] && grep -qF '`ci-ok` needs `build` but no step of it reads `needs.build.result`' <<<"$said"; then
    ok "an aggregate that reads nothing fails"
else
    fail "an aggregate that never reads needs was accepted (status $status):"$'\n'"$said"
fi

# --- 9. The overrides are still read, relative to the caller. -------------
dir="$(caller overrides "$ONE" <<<"$WORKFLOW")"
mv "$dir/.github/workflows/ci.yml" "$dir/.github/workflows/gate.yml"
mv "$dir/.github-guard" "$dir/guard.cfg"
gate "$dir" CI_GATE_WORKFLOW=.github/workflows/gate.yml CI_GATE_GUARD=guard.cfg
if [ "$status" -eq 0 ]; then
    ok "CI_GATE_WORKFLOW and CI_GATE_GUARD are read relative to the caller"
else
    fail "relative overrides were not resolved against the caller (status $status):"$'\n'"$said"
fi

if [ "$fails" -gt 0 ]; then
    echo "test-ci-gate: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  ci-gate holds a caller's ci.yml and .github-guard, and reads the guard as git does"
