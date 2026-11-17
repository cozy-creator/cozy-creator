#!/usr/bin/env bash
# Keep the local Cozy on this checkout's RC branch (cl-1029): follow the branch's head, build, install the CLI, restart Calcifer
# once it is idle, and say when local's TensorD (which Calcifer moves to the installed cozy once this computer is at rest) runs
# it too. Local edits to the checkout build the same way. Requires watchexec, pnpm, Python 3, and the pinned Rust toolchain.
# When Calcifer serves the page from the checkout's Vite (`dev = true` and `[calcifer] development_web`), an edit under web/ is on
# screen at once, builds nothing and restarts nothing.
# Run with --once for one build, --follow for the follower alone. The default follows and watches until stopped.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_TARGET_DIR="${COZY_DEV_TARGET_DIR:-$root/target/dev-local}"
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=1
mkdir -p "$CARGO_TARGET_DIR"
cd "$root"

say() { printf '%s %s\n' "$(date -Is)" "$*"; }

# True when Calcifer serves the page from Vite: it reads these two keys of ~/.cozy/config.toml at its start, as this does at its own
# (crates/calcifer/src/serve.rs). After changing them, restart Calcifer and this.
vite() {
python3 - "$HOME/.cozy/config.toml" <<'PYTHON'
import sys
import tomllib

try:
    with open(sys.argv[1], "rb") as file:
        config = tomllib.load(file)
except (OSError, tomllib.TOMLDecodeError):
    sys.exit(1)
calcifer = config.get("calcifer")
sys.exit(0 if config.get("dev") is True and isinstance(calcifer, dict) and calcifer.get("development_web") else 1)
PYTHON
}

# One look at the branch's head (git can't tell us it moved: one fetch a minute): true when the checkout fast-forwarded to it.
# Only on an rc/* branch, never past a local commit; uncommitted edits come along. Merges in quick succession land as one: it
# waits for the branch to hold still for COZY_DEV_FOLLOW_SETTLE_S, then takes the newest head. A release renames the RC
# (rc/0.25.0 -> rc/0.26.0): it follows the newest rc/X.Y.Z that contains its commit.
skipped=
skip() { [[ $1 == "$skipped" ]] || say "follow: $1"; skipped=$1; return 1; }
# Carried edits go back on the commit the follow started from; if they can't, they stay in $carried and it says so.
put_back() {
    if ! { git reset -q --keep "$was" && git apply "$carried"; }; then
        say "follow: uncommitted edits in ${overlap[*]} couldn't go back; they are in $carried (git apply -3 it)"; return 1
    fi
    rm -f "$carried"
}
follow_once() {
    local branch head next newest was carried overlap=()
    carried=$(git rev-parse --path-format=absolute --git-path cozy-dev-local-carried.patch)
    [[ ! -e $carried ]] \
        || { skip "uncommitted edits from a follow cut short are in $carried: git apply -3 it, then delete it"; return 1; }
    branch=$(git symbolic-ref --short -q HEAD) || { skip "not on a branch"; return 1; }
    [[ $branch == rc/* ]] || { skip "on $branch, not an rc/* branch"; return 1; }
    if ! git fetch -q origin "$branch" 2> /dev/null; then
        newest=$(git ls-remote --heads origin 'rc/*' | sed -n 's#.*refs/heads/\(rc/[0-9]*\.[0-9]*\.[0-9]*\)$#\1#p' | sort -V | tail -1)
        [[ -n $newest && $newest != "$branch" ]] && git fetch -q origin "$newest" && git merge-base --is-ancestor HEAD "origin/$newest" \
            || { skip "origin has no $branch, and no newer RC holds this commit"; return 1; }
        git branch -m "$branch" "$newest" && git branch -q -u "origin/$newest" || return 1
        say "follow: $branch was released; following $newest"
        branch=$newest
    fi
    head=$(git rev-parse "origin/$branch")
    [[ $head != "$(git rev-parse HEAD)" ]] || { skipped=; return 1; }
    git merge-base --is-ancestor HEAD "$head" || { skip "the checkout has commits origin/$branch lacks"; return 1; }
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        sleep "${COZY_DEV_FOLLOW_SETTLE_S:-30}"
        git fetch -q origin "$branch" 2> /dev/null || break
        next=$(git rev-parse "origin/$branch")
        [[ $next != "$head" ]] || break
        head=$next
    done
    # Never under a build reading the tree.
    exec 7>"$CARGO_TARGET_DIR/build.lock"
    flock 7
    was=$(git rev-parse --short HEAD)
    # Uncommitted edits, staged or not, in files the branch changed are carried over three-way (as `git stash` would, without the
    # shared stash): one already merged applies as nothing; one that conflicts leaves the checkout as it was and names the files.
    # Until they are back they wait in $carried, which a follow cut short leaves for the next to name.
    mapfile -t overlap < <(git diff --name-only HEAD "$head" | grep -Fxf <(git diff --name-only HEAD) || true)
    if ((${#overlap[@]})); then
        git diff --binary HEAD -- "${overlap[@]}" > "$carried" \
            || { rm -f "$carried"; exec 7>&-; skip "couldn't save the edits in ${overlap[*]}"; return 1; }
        git restore -q --source=HEAD --staged --worktree -- "${overlap[@]}" \
            || { put_back; exec 7>&-; skip "couldn't set aside the edits in ${overlap[*]}"; return 1; }
    fi
    if ! git merge -q --ff-only "$head"; then
        ((${#overlap[@]})) && put_back
        exec 7>&-; skip "couldn't fast-forward to origin/$branch (git says why above)"; return 1
    fi
    if ((${#overlap[@]})); then
        if ! git apply -3 "$carried" 2> /dev/null || [[ -n $(git diff --name-only --diff-filter=U) ]]; then
            # A file the branch deleted matches nothing here; then nothing was applied either.
            git restore -q --source=HEAD --staged --worktree -- "${overlap[@]}" 2> /dev/null; put_back; exec 7>&-
            skip "origin/$branch conflicts with uncommitted edits in ${overlap[*]}: it follows once those are resolved"; return 1
        fi
        git reset -q -- "${overlap[@]}"; rm -f "$carried"
    fi
    exec 7>&-
    skipped=
    local moved
    moved="$branch $was -> $(git rev-parse --short HEAD) ($(git rev-list --count "$was..HEAD") commits)"
    say "follow: $moved${overlap[*]:+, carrying uncommitted edits in ${overlap[*]}}"
}

if [[ ${1:-} == --follow ]]; then
    while :; do
        if follow_once; then "$root/scripts/dev-local.sh" --once || say "follow: the build failed; the next move or edit tries again"; fi
        sleep "${COZY_DEV_FOLLOW_EVERY_S:-60}"
    done
fi

if [[ ${1:-} != --once ]]; then
    "$root/scripts/dev-local.sh" --follow &
    page=()
    vite || page=(--watch web)
    exec watchexec --debounce 500ms --on-busy-update queue --shell none \
        --watch crates "${page[@]}" --watch vendor --watch scripts/dev-local.sh \
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
if [[ ! -f web/app/dist/index.html || ! -f $page_stamp ]] || { [[ $page_inputs != "$(cat "$page_stamp")" ]] && ! vite; }; then
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

# Calcifer moves to the installed cozy only once it serves nothing (cl-1029), and without a moment off: the next one takes over from it
# (`cozy restart`, cl-1113). One that doesn't start leaves the running one serving, and the binary it ran back in place.
restart() { "$installed" restart; }

# The holder record's pid: the Calcifer that answers (its parent's generation, cl-1113).
serving() { python3 -c 'import json, pathlib; print(json.loads((pathlib.Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())["pid"])'; }

# What runs now, in one line: the checkout's commit, the CLI, Calcifer, and local's TensorD. Calcifer moves local's TensorD to the
# installed cozy once this computer has been at rest (crates/calcifer/src/rc.rs); this waits for its machine.updated at most
# COZY_DEV_TENSORD_WAIT_S, then says what it waits on instead.
versions() {
exec 9>&-
python3 - "$installed" "${COZY_DEV_TENSORD_WAIT_S:-300}" "$(git rev-parse --short HEAD)" "$1" <<'PYTHON'
import json
import subprocess
import sys
import time
import urllib.request
from datetime import datetime
from pathlib import Path

installed, wait, commit, what = sys.argv[1], float(sys.argv[2]), sys.argv[3], sys.argv[4]
cli = subprocess.check_output([installed, "--version"], text=True).strip().removeprefix("cozy ")
holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())


def local() -> dict:
    shown = subprocess.run([installed, "--json", "machine", "show", "local"], capture_output=True, text=True, timeout=60)
    return json.loads(shown.stdout) if shown.returncode == 0 else {}


def tensord(machine: dict) -> str:
    return (machine.get("status") or {}).get("version") or "not running"


machine = local()
waits = tensord(machine) not in (cli, "not running") and not machine.get("held") and machine.get("rc")
if waits and wait > 0:
    deadline = time.monotonic() + wait
    request = urllib.request.Request(
        f"http://{holder['address']}/v1/events",
        headers={"Authorization": f"Bearer {holder['token']}"},
    )
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=30) as stream:
            for raw in stream:
                if time.monotonic() > deadline:
                    break
                line = raw.decode(errors="replace").strip()
                if not line.startswith("data: "):
                    continue
                event = json.loads(line[6:])
                if event.get("type") == "machine.updated" and event.get("machine", {}).get("name") == "local":
                    machine = event["machine"]
                    if tensord(machine) == cli or machine.get("held"):
                        break
    except (OSError, ValueError):
        machine = local() or machine
running = tensord(machine)
said = f"local TensorD {running}"
if machine.get("held"):
    said += f" (held by hand on {machine['held']}: `cozy machine update local` lets it follow again)"
elif running not in (cli, "not running"):
    said += f" ({(machine.get('rc') or {}).get('detail') or 'it moves to the installed cozy at its next update'})"
stamp = datetime.now().astimezone().isoformat(timespec="seconds")
print(f"{stamp} local Cozy {what} at {commit}: CLI {cli} · Calcifer {holder.get('version', '?')} · {said}", flush=True)
PYTHON
}

if cmp --silent "$staged" "$installed"; then
    if ! ready; then
        restart
        ready
    fi
    versions "already matched this build"
    exit 0
fi
# A build that doesn't stay up goes back to the one before it (Paul, 11-16: "swap it from v1 -> v2 so I never have downtime";
# a Calcifer that crashed at every start kept his page down for four minutes): it must answer and keep one process for
# COZY_DEV_STEADY_S, or the previous binary is put back and Calcifer restarted on it. One that never starts never takes over: the
# running one serves on, and its binary is put back (cl-1113).
steady() {
    local pid restarts generation
    pid=$(systemctl --user show cozy-calcifer.service -p MainPID --value)
    restarts=$(systemctl --user show cozy-calcifer.service -p NRestarts --value)
    generation=$(serving)
    for _ in $(seq 1 "${COZY_DEV_STEADY_S:-30}"); do
        sleep 1
        [[ $(systemctl --user show cozy-calcifer.service -p ActiveState --value) == active ]] || return 1
        [[ $(systemctl --user show cozy-calcifer.service -p MainPID --value) == "$pid" ]] || return 1
        [[ $(systemctl --user show cozy-calcifer.service -p NRestarts --value) == "$restarts" ]] || return 1
        # A generation that died is started again under the same parent: the record names another.
        [[ $(serving) == "$generation" ]] || return 1
    done
}
cp --reflink=auto -p "$installed" "$HOME/.cozy/bin/.cozy-previous"
mv -f "$staged" "$installed"
if ! restart; then
    versions "kept: the new build didn't start"
    exit 1
fi
if ! { ready && steady; }; then
    say "the new build didn't stay up (journalctl --user -u cozy-calcifer): back to the one before it"
    # Renamed into place, as the new one was: anything running the installed binary keeps its inode (a copy over it is refused).
    staged=$(mktemp "$HOME/.cozy/bin/.cozy-dev.XXXXXX")
    cp --reflink=auto -p "$HOME/.cozy/bin/.cozy-previous" "$staged"
    mv -f "$staged" "$installed"
    systemctl --user reset-failed cozy-calcifer.service 2> /dev/null || true
    systemctl --user restart cozy-calcifer.service
    ready || true
    versions "rolled back after a failed start"
    exit 1
fi
versions "updated in $((SECONDS - started))s"
