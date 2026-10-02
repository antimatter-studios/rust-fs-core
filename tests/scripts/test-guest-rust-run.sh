#!/usr/bin/env bash
# The family's guest-side Rust layer: inside the test VM it links each
# staged sibling into place, names the toolchain and build directories on
# the guest's own disk, installs the pinned toolchain, and only then runs
# the suite.
#
# Five drivers carried this as the head of their scripts/guest-suite.sh,
# each copy differing only in the filesystem's name (#195). This drives the
# one copy left, `scripts/core.sh guest-rust-run`, from a caller tree laid
# out as the guest lays it out -- the repository, the share beside it, the
# guest's /var -- against a stub rustup, so no toolchain is downloaded.
#
#   bash tests/scripts/test-guest-rust-run.sh
# shellcheck disable=SC2015  # ok() always succeeds: `A && ok || bad` is if/else
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Quiet on a pass, as every test here is: one line at the end, and only the
# checks that failed before it.
fails=0
passed=0
ok() { passed=$((passed + 1)); }
bad() { echo "FAIL  $1" >&2; fails=$((fails + 1)); }

# The toolchain install it runs serialises on flock (util-linux), which
# every Debian guest has. Missing here, the test cannot run: that fails.
command -v flock >/dev/null 2>&1 ||
    { echo "FAIL  flock (util-linux) is required to run this test" >&2; exit 1; }

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/guest-rust-run.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

STUBS="$SANDBOX/stubs"
mkdir -p "$STUBS"

# THE STUB rustup: enough of one for the toolchain install to succeed, or
# to fail on demand. A toolchain is whole when its `.complete` marker exists.
cat > "$STUBS/rustup" <<'EOF'
#!/usr/bin/env bash
set -u
echo "rustup $*" >> "$STUB_LOG"
host=x86_64-unknown-linux-gnu
case "${1:-}" in
    --version) echo "rustup 1.28.2 (stub)" ;;
    toolchain)
        [ -z "${STUB_ALWAYS_FAIL:-}" ] || { echo "error: the network is down (stub)" >&2; exit 1; }
        mkdir -p "$RUSTUP_HOME/toolchains/$3-$host"
        touch "$RUSTUP_HOME/toolchains/$3-$host/.complete"
        ;;
    default) echo "$2" > "$RUSTUP_HOME/default" ;;
    run) echo "cargo 1.95.0 (stub)" ;;
    *) exit 2 ;;
esac
EOF
# THE STUB cargo, the proxy rustup-init puts beside rustup.
cat > "$STUBS/cargo" <<'EOF'
#!/usr/bin/env bash
echo "cargo 1.95.0 (stub)"
EOF
chmod +x "$STUBS/rustup" "$STUBS/cargo"

# THE STUB curl serves a rustup-init that puts both in CARGO_HOME/bin.
cat > "$STUBS/curl" <<EOF
#!/usr/bin/env bash
echo "curl \$*" >> "\$STUB_LOG"
cat <<'INIT'
mkdir -p "\$CARGO_HOME/bin"
cp "$STUBS/rustup" "$STUBS/cargo" "\$CARGO_HOME/bin/"
INIT
EOF
chmod +x "$STUBS/curl"

# THE SUITE: what a driver's guest-suite.sh hands over. It reports where it
# ran, what it was given and the environment it found.
cat > "$SANDBOX/suite" <<'EOF'
#!/usr/bin/env bash
{
    echo "pwd=$(pwd)"
    echo "RUSTUP_HOME=$RUSTUP_HOME"
    echo "CARGO_HOME=$CARGO_HOME"
    echo "CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
    echo "cargo=$(command -v cargo)"
    printf 'args='; printf '[%s]' "$@"; echo
} > "$SUITE_REPORT"
exit "${SUITE_STATUS:-0}"
EOF
chmod +x "$SANDBOX/suite"

n=0
# fresh -- a new guest: the repository, its parent standing in for the
# guest's /, the share with core staged on it, and the guest's /var.
fresh() {
    n=$((n + 1))
    GUEST="$SANDBOX/guest-$n"
    CALLER="$GUEST/repo"
    SHARE="$GUEST/share"
    VAR="$GUEST/var"
    LOG="$GUEST/stub.log"
    REPORT="$GUEST/report"
    mkdir -p "$CALLER/scripts" "$SHARE/siblings/rust-fs-core" "$VAR"
    cp "$REPO/scripts/core.sh" "$CALLER/scripts/core.sh"
    printf '[toolchain]\nchannel = "1.95.0"\n' > "$CALLER/rust-toolchain.toml"
    : > "$LOG"
}
# run [VAR=VALUE...] -- [ARG...] -- the family script from the caller as a
# guest-suite.sh runs it; status in $status, output in $out.
run() {
    local envs=()
    while [ $# -gt 0 ] && [ "$1" != -- ]; do envs+=("$1"); shift; done
    shift
    out="$(cd "$CALLER" && env FS_CORE_ROOT="$REPO" PATH="$STUBS:$PATH" STUB_LOG="$LOG" \
        FLTH_GUEST=1 FS_CORE_GUEST_VAR="$VAR" SUITE_REPORT="$REPORT" "${envs[@]}" \
        bash scripts/core.sh guest-rust-run "$@" 2>&1)"
    status=$?
}
said() { sed -n "s/^$1=//p" "$REPORT" 2>/dev/null; }

# --- the contract ----------------------------------------------------------
v="$(bash "$REPO/scripts/guest-rust-run.sh" --version 2>&1)"
[ "$v" = "rust-fs-core-guest-rust-run 1" ] &&
    ok "it answers --version with its contract" ||
    bad "--version answered '$v'"

# --- a bare guest ----------------------------------------------------------------
fresh
run -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite" --release 'two words' "a'quote"
[ "$status" -eq 0 ] &&
    ok "a bare guest runs the suite" ||
    bad "a bare guest failed (status $status): $out"
[ -L "$GUEST/rust-fs-core" ] && [ "$(readlink "$GUEST/rust-fs-core")" = "$SHARE/siblings/rust-fs-core" ] &&
    ok "the staged sibling is linked where ../rust-fs-core resolves" ||
    bad "the sibling was not linked beside the repository: $(ls -l "$GUEST")"
[ "$(said pwd)" = "$CALLER" ] &&
    ok "the suite runs from the repository" ||
    bad "the suite ran in '$(said pwd)'"
[ "$(said RUSTUP_HOME)" = "$VAR/lib/fs-example-rust/rustup" ] &&
    [ "$(said CARGO_HOME)" = "$VAR/lib/fs-example-rust/cargo" ] &&
    ok "the toolchain lives in /var/lib/<name>-rust, on the guest's own disk" ||
    bad "RUSTUP_HOME '$(said RUSTUP_HOME)', CARGO_HOME '$(said CARGO_HOME)'"
[ "$(said CARGO_TARGET_DIR)" = "$VAR/cache/fs-example-target" ] &&
    ok "and the build in /var/cache/<name>-target" ||
    bad "CARGO_TARGET_DIR '$(said CARGO_TARGET_DIR)'"
[ "$(said cargo)" = "$VAR/lib/fs-example-rust/cargo/bin/cargo" ] &&
    ok "the cargo on PATH is the installed one" ||
    bad "cargo resolved to '$(said cargo)'"
[ -e "$VAR/lib/fs-example-rust/rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/.complete" ] &&
    grep -q '^curl ' "$LOG" &&
    ok "the pinned toolchain was installed first, by the family's install" ||
    bad "no toolchain install ran: $(cat "$LOG")"
[ "$(said args)" = "[--release][two words][a'quote]" ] &&
    ok "the suite gets its arguments exactly" ||
    bad "the suite got '$(said args)'"
grep -q '^== in-guest suite: .*cargo 1.95.0 (stub)$' <<<"$out" &&
    grep -qE '^== in-guest suite: [0-9]+s$' <<<"$out" &&
    ok "it says what ran it and how long it took" ||
    bad "no banner and timing: $out"

# --- a second run on the same guest -------------------------------------------
: > "$LOG"
run -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -eq 0 ] && ! grep -q '^curl ' "$LOG" &&
    [ "$(readlink "$GUEST/rust-fs-core")" = "$SHARE/siblings/rust-fs-core" ] &&
    ok "a second run keeps the link and the toolchain" ||
    bad "a second run: status $status, $(cat "$LOG"): $out"

# --- the suite's status is the run's ---------------------------------------------
run SUITE_STATUS=101 -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -eq 101 ] && grep -qE '^== in-guest suite: [0-9]+s$' <<<"$out" &&
    ok "a failing suite fails the run with its own status, timing still printed" ||
    bad "a failing suite: status $status, '$out'"

# --- a link left pointing somewhere else is pointed back ---------------------
fresh
mkdir -p "$GUEST/elsewhere"
ln -s "$GUEST/elsewhere" "$GUEST/rust-fs-core"
run -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -eq 0 ] && [ "$(readlink "$GUEST/rust-fs-core")" = "$SHARE/siblings/rust-fs-core" ] &&
    ok "a stale link is pointed at the staged sibling" ||
    bad "a stale link was kept (status $status): $(readlink "$GUEST/rust-fs-core")"

# --- refusals ------------------------------------------------------------------
fresh
mkdir -p "$GUEST/rust-fs-core/src"
run -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -ne 0 ] && [ -d "$GUEST/rust-fs-core/src" ] && [ ! -L "$GUEST/rust-fs-core" ] &&
    [ ! -e "$REPORT" ] && grep -q 'rust-fs-core' <<<"$out" &&
    ok "a real directory where the link goes is refused and left alone" ||
    bad "a directory in the link's place: status $status, '$out'"

fresh
run -- fs-example "$SHARE" rust-fs-core rust-lzo1x -- "$SANDBOX/suite"
[ "$status" -ne 0 ] && [ ! -e "$REPORT" ] && grep -q 'rust-lzo1x' <<<"$out" &&
    grep -q 'stage-siblings' <<<"$out" &&
    ok "a sibling that was not staged fails, naming it and what stages it" ||
    bad "an unstaged sibling: status $status, '$out'"

fresh
out="$(cd "$CALLER" && env -u FLTH_GUEST FS_CORE_ROOT="$REPO" PATH="$STUBS:$PATH" STUB_LOG="$LOG" \
    FS_CORE_GUEST_VAR="$VAR" SUITE_REPORT="$REPORT" \
    bash scripts/core.sh guest-rust-run fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite" 2>&1)"
status=$?
[ "$status" -ne 0 ] && grep -q 'FLTH_GUEST' <<<"$out" && [ ! -e "$GUEST/rust-fs-core" ] &&
    [ -z "$(ls -A "$VAR")" ] && [ ! -s "$LOG" ] && [ ! -e "$REPORT" ] &&
    ok "outside the VM it refuses, touching nothing" ||
    bad "outside the VM: status $status, '$out'"

fresh
run STUB_ALWAYS_FAIL=1 -- fs-example "$SHARE" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -ne 0 ] && [ ! -e "$REPORT" ] &&
    ok "a toolchain that will not install stops the run before the suite" ||
    bad "a failed install: status $status, '$out'"

fresh
for args in "fs-example $SHARE rust-fs-core" \
    "fs-example $SHARE rust-fs-core --" \
    "fs-example $SHARE -- suite" \
    "fs/example $SHARE rust-fs-core -- suite" \
    "fs-example $SHARE ../rust-fs-core -- suite" \
    "fs-example $SHARE . -- suite"; do
    # shellcheck disable=SC2086  # the words are the point
    run -- $args
    [ "$status" -eq 2 ] && [ ! -e "$REPORT" ] &&
        ok "'$args' is a usage error" ||
        bad "'$args': status $status, '$out'"
done
run -- fs-example "" rust-fs-core -- "$SANDBOX/suite"
[ "$status" -eq 2 ] && [ ! -e "$REPORT" ] &&
    ok "an empty share is a usage error" ||
    bad "an empty share: status $status, '$out'"

if [ "$fails" -gt 0 ]; then
    echo "test-guest-rust-run: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  the guest's Rust layer is set up before the suite runs ($passed checks)"
