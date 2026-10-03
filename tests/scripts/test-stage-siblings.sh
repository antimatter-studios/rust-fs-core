#!/usr/bin/env bash
# The family's host-side staging puts each path sibling on the VM share at
# the sibling's committed HEAD, and runs the guest command without the
# flag the tier wrapper consumed.
#
# A driver's suite runs in the fs-linux-test-harness VM, which is given the
# repository and not the directory that holds it, so a path dependency on
# ../rust-fs-core has nothing to resolve to there until the sibling is
# staged on the share. Five drivers carried the staging inline in their
# `test:vm` task, each differing only in its name (#195). This drives the
# one copy left, `scripts/core.sh stage-siblings`, from a caller tree with
# real git siblings beside it.
#
#   bash tests/scripts/test-stage-siblings.sh
# shellcheck disable=SC2015  # ok() always succeeds: `A && ok || bad` is if/else
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Quiet on a pass, as every test here is: one line at the end, and only the
# checks that failed before it.
fails=0
passed=0
ok() { passed=$((passed + 1)); }
bad() { echo "FAIL  $1" >&2; fails=$((fails + 1)); }

command -v git >/dev/null 2>&1 ||
    { echo "FAIL  git is required to run this test" >&2; exit 1; }

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/stage-siblings.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null

# The layout every driver has on a developer's machine and on CI: the
# caller, and each path sibling checked out beside it.
WORK="$SANDBOX/work"
CALLER="$WORK/rust-fs-example"
mkdir -p "$CALLER/scripts"
cp "$REPO/scripts/core.sh" "$CALLER/scripts/core.sh"

sibling() {
    local dir="$WORK/$1"
    mkdir -p "$dir/src"
    git -C "$dir" init -q
    printf 'committed %s\n' "$1" > "$dir/src/lib.rs"
    git -C "$dir" add src/lib.rs
    git -C "$dir" commit -q -m one
}
sibling rust-fs-core
sibling rust-lzo1x

SHARE="$SANDBOX/share"
mkdir -p "$SHARE"

# stage [ARG...] -- run the family script from the caller; status in
# $status, output in $out.
stage() {
    out="$(cd "$CALLER" && env FS_CORE_ROOT="$REPO" "$@" 2>&1)"
    status=$?
}

# --- the contract ----------------------------------------------------------
v="$(bash "$REPO/scripts/stage-siblings.sh" --version 2>&1)"
[ "$v" = "rust-fs-core-stage-siblings 1" ] &&
    ok "it answers --version with its contract" ||
    bad "--version answered '$v'"

# --- each sibling, at its HEAD -----------------------------------------------
printf 'uncommitted\n' >> "$WORK/rust-fs-core/src/lib.rs"
printf 'untracked\n' > "$WORK/rust-fs-core/src/new.rs"
stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core rust-lzo1x
[ "$status" -eq 0 ] &&
    ok "two siblings stage" ||
    bad "staging two siblings failed (status $status): $out"
[ "$(cat "$SHARE/siblings/rust-fs-core/src/lib.rs" 2>/dev/null)" = "committed rust-fs-core" ] &&
    [ "$(cat "$SHARE/siblings/rust-lzo1x/src/lib.rs" 2>/dev/null)" = "committed rust-lzo1x" ] &&
    ok "each is staged at siblings/<name> on the share" ||
    bad "the staged files are not the siblings' committed ones: $(find "$SHARE" -type f)"
[ ! -e "$SHARE/siblings/rust-fs-core/src/new.rs" ] && [ ! -e "$SHARE/siblings/rust-fs-core/.git" ] &&
    ok "from HEAD: nothing uncommitted, untracked or of git's own is staged" ||
    bad "the staging carried something HEAD does not: $(find "$SHARE/siblings/rust-fs-core")"
[ -z "$out" ] &&
    ok "staging that works prints nothing" ||
    bad "staging printed: $out"

# --- a sibling staged before is replaced, not merged into ----------------------
git -C "$WORK/rust-fs-core" checkout -q -- src/lib.rs
git -C "$WORK/rust-fs-core" rm -q src/lib.rs
mkdir -p "$WORK/rust-fs-core/lib"
printf 'moved\n' > "$WORK/rust-fs-core/lib/mod.rs"
git -C "$WORK/rust-fs-core" add lib/mod.rs
git -C "$WORK/rust-fs-core" commit -q -m two
stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core
[ "$status" -eq 0 ] && [ -f "$SHARE/siblings/rust-fs-core/lib/mod.rs" ] &&
    [ ! -e "$SHARE/siblings/rust-fs-core/src/lib.rs" ] &&
    ok "a second staging replaces the first: a file the sibling no longer has is gone" ||
    bad "a stale file survived the second staging (status $status): $(find "$SHARE/siblings/rust-fs-core" -type f)"

# --- refusals ------------------------------------------------------------------
stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-missing
[ "$status" -ne 0 ] && grep -q 'rust-fs-missing' <<<"$out" && grep -q 'chore siblings' <<<"$out" &&
    ok "a sibling that is not checked out fails, naming it and what checks it out" ||
    bad "a missing sibling: status $status, '$out'"

mkdir -p "$WORK/rust-fs-empty"
git -C "$WORK/rust-fs-empty" init -q
stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-empty
[ "$status" -ne 0 ] && grep -q 'rust-fs-empty' <<<"$out" &&
    ok "a sibling with no commit to archive fails rather than staging nothing" ||
    bad "a sibling with no HEAD: status $status, '$out'"

stage bash scripts/core.sh stage-siblings "" rust-fs-core
[ "$status" -ne 0 ] && [ ! -e /siblings ] &&
    ok "an empty share is refused, so nothing is removed from /siblings" ||
    bad "an empty share: status $status, '$out'"

stage bash scripts/core.sh stage-siblings "$SANDBOX/no-such-share" rust-fs-core
[ "$status" -ne 0 ] && [ ! -e "$SANDBOX/no-such-share" ] &&
    ok "a share that does not exist is refused, not created" ||
    bad "a missing share: status $status, '$out'"

for name in ../rust-fs-core rust-fs-core/src . ''; do
    stage bash scripts/core.sh stage-siblings "$SHARE" "$name"
    [ "$status" -eq 2 ] &&
        ok "a sibling named '$name' is refused" ||
        bad "a sibling named '$name': status $status, '$out'"
done

stage bash scripts/core.sh stage-siblings "$SHARE"
[ "$status" -eq 2 ] &&
    ok "no sibling at all is a usage error" ||
    bad "no sibling: status $status, '$out'"

# --- then the guest command, with chore's arguments --------------------------
# A command after `--` runs once the siblings are staged, with the words
# chore was given (CLI_ARGS) appended -- less --verbose and -v, which are
# the tier wrapper's and would mean something else to the guest's cargo.
cat > "$SANDBOX/guest-test" <<'EOF'
#!/usr/bin/env bash
[ -f "$STAGED" ] || { echo "ran before staging"; exit 9; }
printf '[%s]' "$@"
exit "${GUEST_STATUS:-0}"
EOF
chmod +x "$SANDBOX/guest-test"
stage STAGED="$SHARE/siblings/rust-fs-core/lib/mod.rs" CLI_ARGS="--verbose reads -v --nocapture" \
    bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core -- "$SANDBOX/guest-test" guest-test
[ "$status" -eq 0 ] && [ "$out" = "[guest-test][reads][--nocapture]" ] &&
    ok "the command runs after staging, with CLI_ARGS less --verbose and -v" ||
    bad "the command: status $status, '$out'"

# A test filter is a word, not a pattern for the host's shell to expand
# against whatever the caller's directory holds.
touch "$CALLER/matches-a-glob"
stage STAGED="$SHARE/siblings/rust-fs-core/lib/mod.rs" CLI_ARGS="read*" \
    bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core -- "$SANDBOX/guest-test"
[ "$status" -eq 0 ] && [ "$out" = "[read*]" ] &&
    ok "a word in CLI_ARGS reaches the command unglobbed" ||
    bad "CLI_ARGS was globbed: status $status, '$out'"
touch "$CALLER/read-me"
stage STAGED="$SHARE/siblings/rust-fs-core/lib/mod.rs" CLI_ARGS="read*" \
    bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core -- "$SANDBOX/guest-test"
[ "$status" -eq 0 ] && [ "$out" = "[read*]" ] &&
    ok "even when a file in the caller matches it" ||
    bad "CLI_ARGS was globbed against the caller: status $status, '$out'"

stage STAGED="$SHARE/siblings/rust-fs-core/lib/mod.rs" GUEST_STATUS=101 \
    bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core -- "$SANDBOX/guest-test"
[ "$status" -eq 101 ] && [ "$out" = "[]" ] &&
    ok "the command's status is the run's, and no CLI_ARGS means no arguments" ||
    bad "the command's status: $status, '$out'"

stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-missing -- "$SANDBOX/guest-test"
[ "$status" -ne 0 ] && ! grep -q 'ran before staging' <<<"$out" && ! grep -q '^\[' <<<"$out" &&
    ok "a staging that fails does not run the command" ||
    bad "the command ran after a failed staging: status $status, '$out'"

stage bash scripts/core.sh stage-siblings "$SHARE" rust-fs-core --
[ "$status" -eq 2 ] &&
    ok "a -- with no command after it is a usage error" ||
    bad "an empty command: status $status, '$out'"

if [ "$fails" -gt 0 ]; then
    echo "test-stage-siblings: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  the siblings are staged at HEAD, and the guest command runs after ($passed checks)"
