#!/usr/bin/env bash
# Watch this checkout and install successful incremental builds into the local Cozy.
# Requires watchexec, pnpm, Python 3, and the pinned Rust toolchain.
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
        --watch scripts/prepare-securefs.py --watch scripts/restart-calcifer-when-idle.py \
        --watch Cargo.toml --watch Cargo.lock --watch rust-toolchain.toml \
        --ignore '**/target/**' --ignore '**/node_modules/**' --ignore '**/dist/**' \
        --ignore '**/__pycache__/**' --ignore '**/*.pyc' \
        -- "$root/scripts/dev-local.sh" --once
fi

exec 8>"$CARGO_TARGET_DIR/build.lock"
flock 8
started=$SECONDS
printf '\n%s building local Cozy from %s\n' "$(date -Is)" "$root"

# Embed the same checksum-pinned filesystem provider as published releases. The
# preparation script reuses the verified archive on subsequent incremental builds.
host_target=$(rustc -vV | sed -n 's/^host: //p')
export COZY_SECUREFS_ARCHIVE
COZY_SECUREFS_ARCHIVE=$(python3 scripts/prepare-securefs.py \
    --target "$host_target" --out-dir "$CARGO_TARGET_DIR/vault-helper")
export COZY_SECUREFS_REQUIRED=1

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

# Replace the CLI at once, and restart Calcifer once it serves nothing (cl-962): an upload, a publish, a
# submission or a page's write in flight holds it up to 10 min, then it restarts anyway and says so. A model
# running on the local worker does not hold it. Existing processes keep their inode.
install_lock="${COZY_DEV_INSTALL_LOCK:-$HOME/.cozy/dev-local-install.lock}"
exec 9>"$install_lock"
flock 9
ready() {
python3 - "$installed" <<'PYTHON'
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import urlsplit
from pathlib import Path

binary = Path(sys.argv[1])
deadline = time.monotonic() + 30
holder_file = Path.home() / ".cozy/calcifer/calcifer.lock.holder"
while time.monotonic() < deadline:
    try:
        holder = json.loads(holder_file.read_text())
        running = Path(f"/proc/{holder['pid']}/exe")
        address = urlsplit(f"http://{holder['address']}")
        if address.hostname not in ("127.0.0.1", "::1"):
            raise SystemExit("Calcifer's record does not name a loopback address")
        if running.samefile(binary):
            request = urllib.request.Request(
                f"http://{holder['address']}/v1/status",
                headers={"Authorization": f"Bearer {holder['token']}"},
            )
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=2) as response:
                json.load(response)
            break
    except (OSError, KeyError, ValueError, urllib.error.URLError):
        pass
    service = subprocess.check_output(
        ["systemctl", "--user", "show", "cozy-calcifer.service", "-p", "ActiveState", "--value"],
        text=True,
    ).strip()
    if service in ("inactive", "failed"):
        raise SystemExit(f"Calcifer service is {service}; see journalctl --user -u cozy-calcifer")
    time.sleep(0.25)
else:
    raise SystemExit("Calcifer readiness is unconfirmed after 30s; further source edits can retry")
PYTHON
}

restart() { python3 "$root/scripts/restart-calcifer-when-idle.py" --wait "${COZY_DEV_RESTART_WAIT_S:-600}"; }
if cmp --silent "$staged" "$installed"; then
    if ! ready; then
        restart
        ready
    fi
    printf '%s installed Cozy already matches this build\n' "$(date -Is)"
    exit 0
fi
cp --reflink=auto -p "$installed" "$HOME/.cozy/bin/.cozy-previous"
mv -f "$staged" "$installed"
restart
ready
printf '%s local CLI and Calcifer updated in %ss\n' "$(date -Is)" "$((SECONDS - started))"
"$installed" --version
