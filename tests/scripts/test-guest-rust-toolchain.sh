#!/usr/bin/env bash
# The family's guest toolchain install recovers from the wreckage an
# interrupted install leaves behind.
#
# Every Linux driver in the family runs its suite inside the
# fs-linux-test-harness VM, and installs the toolchain its
# rust-toolchain.toml pins there first. That install lives on the guest's
# own disk, which outlives `vm:down`, so an install a reaper or a deadline
# stopped half way through is still there on the next run -- and rustup will
# not finish it. Five drivers carried a copy of the install and only one of
# them recovered (#190). This drives the one copy left, scripts/core.sh
# guest-rust-toolchain, through each kind of wreckage, against a stub rustup
# that refuses exactly the way the real one was measured refusing.
#
#   bash tests/scripts/test-guest-rust-toolchain.sh
# shellcheck disable=SC2015  # ok() always succeeds: `A && ok || bad` is if/else
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Quiet on a pass, as every test here is: one line at the end, and only the
# checks that failed before it.
fails=0
passed=0
ok() { passed=$((passed + 1)); }
bad() { echo "FAIL  $1" >&2; fails=$((fails + 1)); }

# flock serialises two suites sharing one guest; it is util-linux, which
# every Debian guest has. Missing here, the test cannot run: that fails.
command -v flock >/dev/null 2>&1 ||
    { echo "FAIL  flock (util-linux) is required to run this test" >&2; exit 1; }

mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/guest-rust-toolchain.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

STUBS="$SANDBOX/stubs"
mkdir -p "$STUBS"

# THE STUB rustup. State lives in RUSTUP_HOME, as the real one's does, and
# it fails the two ways an interrupted install was measured failing on the
# guest: a `.partial` left in downloads/ ("could not rename 'downloaded'
# file"), and a toolchain directory half populated ("detected conflict").
# A toolchain is whole when its `.complete` marker exists.
cat > "$STUBS/rustup" <<'EOF'
#!/usr/bin/env bash
set -u
echo "rustup $*" >> "$STUB_LOG"
host=x86_64-unknown-linux-gnu
case "${1:-}" in
    --version) echo "rustup 1.28.2 (stub)" ;;
    toolchain)
        [ "${2:-}" = install ] || exit 2
        t="$3"; shift 3
        if [ -n "${STUB_ALWAYS_FAIL:-}" ]; then
            echo "error: the network is down (stub)" >&2
            exit 1
        fi
        for p in "$RUSTUP_HOME"/downloads/*.partial; do
            [ -e "$p" ] || continue
            echo "error: could not rename 'downloaded' file from '$p' to '${p%.partial}': No such file or directory (os error 2)" >&2
            exit 1
        done
        dir="$RUSTUP_HOME/toolchains/$t-$host"
        if [ -d "$dir" ] && [ ! -e "$dir/.complete" ]; then
            echo "error: failed to install component: 'rust-gdb', detected conflict: 'bin/rust-gdb'" >&2
            exit 1
        fi
        # Two installs at once in one RUSTUP_HOME tread on each other.
        mkdir "$RUSTUP_HOME/.stub-busy" 2>/dev/null ||
            { echo "error: another install is running in this RUSTUP_HOME (stub)" >&2; exit 1; }
        sleep "${STUB_INSTALL_SECONDS:-0}"
        mkdir -p "$dir/bin" "$RUSTUP_HOME/downloads" "$RUSTUP_HOME/tmp"
        echo "$*" > "$dir/.args"
        touch "$dir/.complete"
        rmdir "$RUSTUP_HOME/.stub-busy"
        ;;
    default) echo "$2" > "$RUSTUP_HOME/default" ;;
    run)
        [ -e "$RUSTUP_HOME/toolchains/$2-$host/.complete" ] ||
            { echo "error: toolchain '$2' is not installed" >&2; exit 1; }
        echo "cargo 1.95.0 (stub)"
        ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$STUBS/rustup"

# THE STUB curl serves a rustup-init that puts the stub rustup in place.
cat > "$STUBS/curl" <<EOF
#!/usr/bin/env bash
echo "curl \$*" >> "\$STUB_LOG"
cat <<'INIT'
echo "rustup-init \$*" >> "\$STUB_LOG"
mkdir -p "\$CARGO_HOME/bin"
cp "$STUBS/rustup" "\$CARGO_HOME/bin/rustup"
chmod +x "\$CARGO_HOME/bin/rustup"
INIT
EOF
chmod +x "$STUBS/curl"

# A caller: a repository carrying scripts/core.sh and a rust-toolchain.toml,
# the way every driver does.
CALLER="$SANDBOX/caller"
mkdir -p "$CALLER/scripts"
cp "$REPO/scripts/core.sh" "$CALLER/scripts/core.sh"
printf '[toolchain]\nchannel = "1.95.0"\ncomponents = ["rustfmt", "clippy"]\n' \
    > "$CALLER/rust-toolchain.toml"

n=0
# install [VAR=VALUE...] -- run the install into a fresh guest home, or into
# the home named by HOME_DIR when set; status in $status, output in $out.
install() {
    out="$(env FS_CORE_ROOT="$REPO" PATH="$STUBS:$PATH" STUB_LOG="$LOG" \
        RUSTUP_HOME="$HOME_DIR/rustup" CARGO_HOME="$HOME_DIR/cargo" "$@" \
        bash "$CALLER/scripts/core.sh" guest-rust-toolchain 2>&1)"
    status=$?
}
fresh() {
    n=$((n + 1))
    HOME_DIR="$SANDBOX/guest-$n"
    LOG="$SANDBOX/log-$n"
    : > "$LOG"
}
TC=1.95.0-x86_64-unknown-linux-gnu

# --- the contract ----------------------------------------------------------
v="$(bash "$REPO/scripts/guest-rust-toolchain.sh" --version 2>&1)"
[ "$v" = "rust-fs-core-guest-rust-toolchain 1" ] &&
    ok "it answers --version with its contract" ||
    bad "--version answered '$v'"

# --- a guest with nothing installed ----------------------------------------
fresh
install
if [ "$status" -eq 0 ] && [ -e "$HOME_DIR/rustup/toolchains/$TC/.complete" ]; then
    ok "a bare guest gets rustup and the pinned toolchain"
else
    bad "a bare guest was not provisioned (status $status): $out"
fi
grep -q '^curl .*https://sh.rustup.rs' "$LOG" &&
    grep -q '^rustup-init .*--default-toolchain none' "$LOG" &&
    ok "rustup comes from sh.rustup.rs with no toolchain of its own" ||
    bad "rustup was not installed from sh.rustup.rs: $(cat "$LOG")"
args="$(cat "$HOME_DIR/rustup/toolchains/$TC/.args" 2>/dev/null)"
case "$args" in
    *"--component rustfmt"*"--component clippy"*"--profile minimal"*)
        ok "with rustfmt and clippy on the minimal profile" ;;
    *) bad "the toolchain was installed with '$args'" ;;
esac
[ "$(cat "$HOME_DIR/rustup/default" 2>/dev/null)" = 1.95.0 ] &&
    ok "and the pinned channel is the default" ||
    bad "the default toolchain is not 1.95.0"

# --- a second run over a whole install --------------------------------------
: > "$LOG"
install
[ "$status" -eq 0 ] && ! grep -q '^curl ' "$LOG" &&
    ok "a whole install is kept: a second run fetches no rustup" ||
    bad "a second run failed or fetched rustup again (status $status): $out"

# --- an install interrupted mid-download ------------------------------------
fresh
install
mkdir -p "$HOME_DIR/rustup/downloads"
printf 'half' > "$HOME_DIR/rustup/downloads/0123abcd.partial"
install
[ "$status" -eq 0 ] &&
    ok "an install interrupted mid-download leaves nothing the next run trips on" ||
    bad "a leftover .partial broke the next run (status $status): $out"

# --- a toolchain left half installed ----------------------------------------
fresh
mkdir -p "$HOME_DIR/rustup/toolchains/$TC/bin"
install
if [ "$status" -eq 0 ] && [ -e "$HOME_DIR/rustup/toolchains/$TC/.complete" ]; then
    ok "a half-installed toolchain is removed and installed again"
else
    bad "a half-installed toolchain broke the run (status $status): $out"
fi
grep -q 'detected conflict' <<<"$out" &&
    ok "and the first attempt's error is shown, not swallowed" ||
    bad "the first attempt's error was not shown: $out"

# --- a rustup that rustup-init never finished writing -----------------------
fresh
mkdir -p "$HOME_DIR/cargo/bin"
printf '\177EL' > "$HOME_DIR/cargo/bin/rustup"
chmod +x "$HOME_DIR/cargo/bin/rustup"
install
[ "$status" -eq 0 ] && grep -q '^curl ' "$LOG" &&
    ok "a truncated rustup is installed again" ||
    bad "a truncated rustup broke the run (status $status): $out"

# --- an install that keeps failing fails, after one retry -------------------
fresh
install STUB_ALWAYS_FAIL=1
attempts="$(grep -c '^rustup toolchain install' "$LOG")"
[ "$status" -ne 0 ] && [ "$attempts" -eq 2 ] &&
    ok "an install that keeps failing fails after exactly one retry" ||
    bad "a failing install: status $status after $attempts attempts: $out"

# --- two suites sharing one guest -------------------------------------------
fresh
(install STUB_INSTALL_SECONDS=1; echo "$status" > "$SANDBOX/first") &
(install STUB_INSTALL_SECONDS=1; echo "$status" > "$SANDBOX/second") &
wait
[ "$(cat "$SANDBOX/first")" = 0 ] && [ "$(cat "$SANDBOX/second")" = 0 ] &&
    ok "two installs into one guest at once are serialised" ||
    bad "two concurrent installs collided: $(cat "$SANDBOX/first") $(cat "$SANDBOX/second")"

# --- refusals ----------------------------------------------------------------
fresh
out="$(env -u RUSTUP_HOME FS_CORE_ROOT="$REPO" PATH="$STUBS:$PATH" STUB_LOG="$LOG" \
    CARGO_HOME="$HOME_DIR/cargo" bash "$CALLER/scripts/core.sh" guest-rust-toolchain 2>&1)"
status=$?
[ "$status" -ne 0 ] && grep -q RUSTUP_HOME <<<"$out" && [ ! -s "$LOG" ] &&
    ok "it refuses to run without RUSTUP_HOME, touching nothing" ||
    bad "no RUSTUP_HOME: status $status, '$out'"

fresh
printf '[toolchain]\ncomponents = ["rustfmt"]\n' > "$CALLER/rust-toolchain.toml"
install
[ "$status" -ne 0 ] && grep -q 'rust-toolchain.toml' <<<"$out" &&
    ok "it refuses a rust-toolchain.toml that pins no channel" ||
    bad "no channel: status $status, '$out'"

if [ "$fails" -gt 0 ]; then
    echo "test-guest-rust-toolchain: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  the guest toolchain install recovers from an interrupted one ($passed checks)"
