#!/usr/bin/env bash
# ci.yml can be started by hand on any branch, and a run started that way is
# the full pipeline. Without `workflow_dispatch` that run is refused; with it
# but with the `changes` job answering `false`, or a job gated on the event,
# the run is green for having tested nothing -- which reads exactly like a
# full pipeline that passed.
#
#   bash tests/scripts/test-ci-dispatch.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CI="$REPO/.github/workflows/ci.yml"

fails=0
ok() { echo "ok    $*"; }
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

[ -f "$CI" ] || { echo "FAIL  .github/workflows/ci.yml is missing" >&2; exit 1; }

# The triggers: the keys indented one level under the top-level `on:`.
triggers="$(awk '
    /^on:/ { inside = 1; next }
    inside && /^[^ #]/ { exit }
    inside && /^  [a-z_]+:/ { sub(/^  /, ""); sub(/:.*/, ""); print }
' "$CI")"

for want in pull_request merge_group workflow_dispatch; do
    if grep -qx "$want" <<<"$triggers"; then
        ok "ci.yml runs on $want"
    else
        fail "ci.yml does not run on $want (its triggers: $(tr '\n' ' ' <<<"$triggers"))"
    fi
done

# The `changes` job's classifier, run as a dispatch would run it. A dispatch
# carries no pull request, merge group or push range, so the classifier has
# nothing to diff and must answer `true`: the full pipeline. This executes
# the script in ci.yml rather than reading it, so it holds whatever the
# script becomes.
classify="$(awk '
    /^      - name: Classify the change/ { inside = 1; next }
    inside && /^        run: \|/ { body = 1; next }
    body && /^          / { sub(/^          /, ""); print; next }
    body && /^$/ { print; next }
    body { exit }
' "$CI")"

if ! grep -q 'GITHUB_OUTPUT' <<<"$classify"; then
    fail "the changes job's classifier was not found in ci.yml (step \"Classify the change\")"
else
    sandbox="$(mktemp -d)"
    trap 'rm -rf "$sandbox"' EXIT
    out="$sandbox/github-output"
    : > "$out"
    (
        cd "$sandbox" &&
            EVENT=workflow_dispatch REF=refs/heads/some-branch \
            PR_BASE='' PR_HEAD='' MQ_BASE='' MQ_HEAD='' PUSH_BEFORE='' PUSH_AFTER='' \
            GITHUB_OUTPUT="$out" bash -c "$classify"
    ) > "$sandbox/log" 2>&1
    status=$?
    if [ "$status" -eq 0 ] && grep -qx 'code=true' "$out"; then
        ok "a dispatch is classified as needing the full pipeline"
    else
        fail "a dispatch is not classified code=true (status $status, output: $(cat "$out"), log: $(cat "$sandbox/log"))"
    fi
fi

# A job gated on the event can drop a dispatch from the pipeline while every
# check above still passes. None is gated that way today; one that needs to
# be must extend this test to prove a dispatch still runs it.
gated="$(grep -n '^ *if:.*github\.event_name' "$CI" || true)"
if [ -z "$gated" ]; then
    ok "no job or step is gated on github.event_name"
else
    fail "ci.yml gates on github.event_name, which can skip a dispatch: $gated"
fi

if [ "$fails" -gt 0 ]; then
    echo "test-ci-dispatch: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  ci.yml can be dispatched on any branch, and a dispatch runs the full pipeline"
