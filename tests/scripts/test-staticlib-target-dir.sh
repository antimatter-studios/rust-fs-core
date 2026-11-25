#!/usr/bin/env bash
# Regression for #430: staticlib must copy the archive Cargo built, even when
# Cargo's target directory comes from the environment or Cargo configuration.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
command -v chore >/dev/null || { echo "FAIL  chore is required for this test" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL  python3 is required for this test" >&2; exit 1; }
mkdir -p "$REPO/tmp"
SANDBOX="$(mktemp -d "$REPO/tmp/staticlib-target-dir.XXXXXX")"
trap 'rm -rf "$SANDBOX"' EXIT HUP INT TERM

run_case() {
    local name="$1" setting="$2" value="$3" root="$SANDBOX/$1" fakebin
    mkdir -p "$root" "$root/src" "$root/tests" "$root/include" "$root/target/aarch64-apple-darwin/release"
    cp "$REPO/chores.yml" "$REPO/Cargo.toml" "$root/"
    cp "$REPO/tests/header_names_the_built_library.rs" "$root/tests/"
    cp "$REPO/include/fs_core.h" "$root/include/"
    printf 'pub fn fixture() {}\n' > "$root/src/lib.rs"
    fakebin="$root/fakebin"
    mkdir -p "$fakebin"
    cat > "$fakebin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == test ]]; then exit 0; fi
if [[ "${1:-}" == metadata ]]; then
    python3 - "$PWD" <<'PY'
import json, os, re, sys
root = sys.argv[1]
target = os.environ.get("CARGO_TARGET_DIR")
if target is None:
    try:
        config = open(os.path.join(root, ".cargo", "config.toml")).read()
    except FileNotFoundError:
        config = ""
    match = re.search(r'^\s*target-dir\s*=\s*["\']([^"\']+)', config, re.M)
    target = match.group(1) if match else "target"
print(json.dumps({"target_directory": os.path.abspath(os.path.join(root, target))}))
PY
    exit 0
fi
if [[ "${1:-}" == build ]]; then
    target_dir="$(python3 - "$PWD" <<'PY'
import json, os, re, sys
root = sys.argv[1]
target = os.environ.get("CARGO_TARGET_DIR")
if target is None:
    try: config = open(os.path.join(root, ".cargo", "config.toml")).read()
    except FileNotFoundError: config = ""
    match = re.search(r'^\s*target-dir\s*=\s*["\']([^"\']+)', config, re.M)
    target = match.group(1) if match else "target"
print(os.path.abspath(os.path.join(root, target)))
PY
)"
    mkdir -p "$target_dir/aarch64-apple-darwin/release"
    printf 'fresh-%s\n' "$FIXTURE_CASE" > "$target_dir/aarch64-apple-darwin/release/libfs_core.a"
    exit 0
fi
echo "unexpected cargo invocation: $*" >&2
exit 2
SH
    cat > "$fakebin/rustup" <<'SH'
#!/usr/bin/env bash
exit 0
SH
    chmod +x "$fakebin/cargo" "$fakebin/rustup"
    mkdir -p "$root/dist"
    printf 'stale-default-target\n' > "$root/target/aarch64-apple-darwin/release/libfs_core.a"
    if [[ "$setting" == config ]]; then
        mkdir -p "$root/.cargo"
        printf '[build]\ntarget-dir = "%s"\n' "$value" > "$root/.cargo/config.toml"
    fi
    (
        cd "$root"
        export PATH="$fakebin:$PATH" FIXTURE_CASE="$name"
        unset CARGO_TARGET_DIR
        [[ "$setting" != env ]] || export CARGO_TARGET_DIR="$value"
        chore --force staticlib >/dev/null
    )
    local got
    got="$(cat "$root/dist/libfs_core.a")"
    [[ "$got" == "fresh-$name" ]] || {
        echo "FAIL  $name: dist contains '$got', expected 'fresh-$name'" >&2
        return 1
    }
}

run_case default unset ''
run_case env-relative env custom-target
run_case env-absolute env "$SANDBOX/absolute-target"
run_case configured config configured-target

echo 'PASS  staticlib copies the archive from Cargo target_directory'
