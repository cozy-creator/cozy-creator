#!/usr/bin/env bash
# Watch this checkout and install successful incremental builds into the local Cozy.
# Requires watchexec, pnpm, and the Rust toolchain pinned by rust-toolchain.toml.
# Run with --once for one build. The default watches until stopped.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_TARGET_DIR="${COZY_DEV_TARGET_DIR:-$root/target/dev-local}"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=1
mkdir -p "$CARGO_TARGET_DIR"
cd "$root"

if [[ ${1:-} != --once ]]; then
    exec watchexec --debounce 500ms --on-busy-update queue --shell none \
        --watch crates --watch web --watch vendor --watch scripts/dev-local.sh \
        --watch Cargo.toml --watch Cargo.lock --watch rust-toolchain.toml \
        --ignore '**/target/**' --ignore '**/node_modules/**' --ignore '**/dist/**' \
        --ignore '**/__pycache__/**' --ignore '**/*.pyc' \
        -- "$root/scripts/dev-local.sh" --once
fi

exec 8>"$CARGO_TARGET_DIR/build.lock"
flock 8
started=$SECONDS
printf '\n%s building local Cozy from %s\n' "$(date -Is)" "$root"

# Hash names and contents so deletion, rename and checkout also rebuild the page.
page_stamp="$CARGO_TARGET_DIR/page-built"
page_inputs=$(git ls-files -z --cached --others --exclude-standard -- web | sort -zu | \
    while IFS= read -r -d '' file; do
        if [[ -f $file ]]; then sha256sum -- "$file"; fi
    done | sha256sum | cut -d ' ' -f 1)
if [[ ! -f web/app/dist/index.html || ! -f $page_stamp ]] || \
    [[ $page_inputs != "$(cat "$page_stamp")" ]]; then
    web/build.sh
    printf '%s\n' "$page_inputs" > "$page_stamp"
fi

cargo build --locked --jobs "${COZY_DEV_JOBS:-4}" --package cozy
binary="$CARGO_TARGET_DIR/cozy"
strip --strip-all -o "$binary" "$CARGO_TARGET_DIR/debug/cozy"
installed="$HOME/.cozy/bin/cozy"
"$binary" --version

# Keep a stable install candidate while other checks use the build directory.
staged=$(mktemp "$HOME/.cozy/bin/.cozy-dev.XXXXXX")
trap 'rm -f "$staged"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
install -m 755 "$binary" "$staged"
flock -u 8
exec 8>&-

# Replace the CLI and restart Calcifer directly, so a model running on the local
# worker does not delay interactive changes. Existing processes keep their inode.
install_lock="${COZY_DEV_INSTALL_LOCK:-$HOME/.cozy/dev-local-install.lock}"
exec 9>"$install_lock"
flock 9
if cmp --silent "$staged" "$installed"; then
    printf '%s installed Cozy already matches this build\n' "$(date -Is)"
    exit 0
fi
cp --reflink=auto -p "$installed" "$HOME/.cozy/bin/.cozy-previous"
mv -f "$staged" "$installed"
systemctl --user restart cozy-calcifer.service
python3 - "$installed" <<'PYTHON'
import hashlib
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

binary = Path(sys.argv[1])
expected = hashlib.sha256(binary.read_bytes()).digest()
holder_file = Path.home() / ".cozy/calcifer/calcifer.lock.holder"
while True:
    try:
        holder = json.loads(holder_file.read_text())
        running = Path(f"/proc/{holder['pid']}/exe")
        if hashlib.sha256(running.read_bytes()).digest() == expected:
            request = urllib.request.Request(
                f"http://{holder['address']}/v1/status",
                headers={"Authorization": f"Bearer {holder['token']}"},
            )
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=2) as response:
                json.load(response)
            break
    except (OSError, KeyError, ValueError, urllib.error.URLError):
        pass
    service = subprocess.run(
        ["systemctl", "--user", "is-failed", "--quiet", "cozy-calcifer.service"],
        check=False,
    )
    if service.returncode == 0:
        raise SystemExit("Calcifer failed to start; see journalctl --user -u cozy-calcifer")
    time.sleep(0.25)
PYTHON
printf '%s local CLI and Calcifer updated in %ss\n' "$(date -Is)" "$((SECONDS - started))"
"$installed" --version
