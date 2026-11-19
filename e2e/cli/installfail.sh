# e2e/cli/installfail.sh: an install's failure judged by what TensorD reads itself, never uv's words (xs-220, hack audit tensord #5).

# install_failures: three staged failures of a package install on local, each judged from a fact.
# - A lock for another architecture (aarch64 here on x86_64): refused `release_platform_unsupported` before an environment is made,
#   naming where it runs. Before, uv made the environment and its words were matched ("not compatible with the lockfile's supported
#   environments"), naming this machine's platform instead.
# - A dependency whose host doesn't answer the machine (its cgroup refused the find-links server): `hub_unreachable`, asked again
#   naming the host; once the host answers again the install completes.
# - A disk that a dependency's build fills (to ENOSPC, held a moment, then deleted) before printing 120 KB more: `machine_disk_full`.
#   Before, the words were matched in the kept 64 KiB tail of uv's stderr, where they no longer were: `package_installation_uv_failed`.
install_failures() (
  step "an install's failure is judged from the machine's own facts: platform, network, disk (xs-220)"
  # It fills the disk it builds on and adds an iptables rule: only on a GitHub-hosted runner's own VM.
  [ "${RUNNER_ENVIRONMENT:-}" = github-hosted ] && [ "${RUNNER_OS:-}" = Linux ] && [ "$(uname -m)" = x86_64 ] \
    || { echo "install failures: GitHub-hosted x86_64 Linux runners only (it fills the disk and adds an iptables rule)"; exit 0; }
  local root=$OUT/install-failures project code server port
  project=$root/project
  mkdir -p "$root"
  trap 'kill "${server:-}" 2> /dev/null || true' EXIT
  cozy --json new --unencrypted "$project" > "$root/new.json" 2>&1 || { check "install failures: cozy new: $(tail -c 300 "$root/new.json")"; exit 0; }
  # package <name> <dir>: weightless as `e2e-<name>` at <dir>, with `extra` appended to its pyproject.toml.
  package() {
    cp -r "$here/weightless" "$2"
    sed -i "s/^name = \"e2e-weightless\"/name = \"e2e-$1\"/" "$2/pyproject.toml"
    printf '%s\n' "${3:-}" >> "$2/pyproject.toml"
  }
  install() { code=0; (cd "$project" && cozy --json package install "$2" --on local) > "$root/$1.ndjson" 2> "$root/$1.err" || code=$?; }
  said() { cat "$root/$1.ndjson" "$root/$1.err" 2> /dev/null; }

  # 1. Another architecture's lock.
  package elsewhere "$root/elsewhere" $'\n[tool.uv]\nenvironments = ["sys_platform == \'linux\' and platform_machine == \'aarch64\'"]'
  uv lock --project "$root/elsewhere" > "$root/elsewhere-lock.log" 2>&1 || { check "install failures: locking for aarch64: $(tail -c 300 "$root/elsewhere-lock.log")"; exit 0; }
  install elsewhere "$root/elsewhere"
  [ "$code" != 0 ] && said elsewhere | grep -q release_platform_unsupported && said elsewhere | grep -q "runs on Linux aarch64, not this machine" \
    || check "install failures: an aarch64 lock wasn't refused naming where it runs: $(said elsewhere | tail -c 400)"
  ! said elsewhere | grep -q "creating the environment" || check "install failures: an aarch64 lock made an environment before it was refused"

  # 2. A dependency's host that doesn't answer the machine, then does. Calcifer locks the package itself (its `uv lock` reads the
  # find-links page), so the host is up for it; only the machine's own processes (its cgroup) are refused it, as a machine whose
  # network dropped would be.
  mkdir -p "$root/netdep/e2e_netdep" "$root/served"
  printf '[build-system]\nrequires = ["uv_build>=0.12.7,<0.13"]\nbuild-backend = "uv_build"\n\n[project]\nname = "e2e-netdep"\nversion = "0.1.0"\nrequires-python = ">=3.12"\n\n[tool.uv.build-backend]\nmodule-name = "e2e_netdep"\nmodule-root = ""\n' > "$root/netdep/pyproject.toml"
  : > "$root/netdep/e2e_netdep/__init__.py"
  uv build --wheel "$root/netdep" --out-dir "$root/served" > "$root/netdep-build.log" 2>&1 || { check "install failures: its served wheel: $(tail -c 300 "$root/netdep-build.log")"; exit 0; }
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  python3 -m http.server "$port" --bind 127.0.0.1 --directory "$root/served" > "$root/served.log" 2>&1 & server=$!
  sleep 1
  package reach "$root/reach" $'\n[tool.uv]\nfind-links = ["http://127.0.0.1:'"$port"'/"]'
  sed -i 's/^dependencies = \[/dependencies = ["e2e-netdep", /' "$root/reach/pyproject.toml"
  uv lock --project "$root/reach" > "$root/reach-lock.log" 2>&1 || { check "install failures: locking against the served wheel: $(tail -c 300 "$root/reach-lock.log")"; exit 0; }
  # The machine's cgroup, as TensorD says it at its start.
  local cgroup
  cgroup=$(cozy machine logs local tensord -n 0 2> /dev/null | grep -o 'contained in /sys/fs/cgroup/[^ ]*' | tail -1 | sed 's|contained in /sys/fs/cgroup/||')
  [ -n "$cgroup" ] || { check "install failures: TensorD named no cgroup it runs in"; exit 0; }
  sudo iptables -w -I OUTPUT -p tcp -d 127.0.0.1 --dport "$port" -m cgroup --path "$cgroup" -j REJECT --reject-with tcp-reset \
    || { check "install failures: refusing the machine the served host (cgroup $cgroup)"; exit 0; }
  trap 'kill "${server:-}" 2> /dev/null || true; sudo iptables -w -D OUTPUT -p tcp -d 127.0.0.1 --dport "'"$port"'" -m cgroup --path "'"$cgroup"'" -j REJECT --reject-with tcp-reset 2> /dev/null || true' EXIT
  (cd "$project" && cozy --json package install "$root/reach" --on local) > "$root/reach.ndjson" 2> "$root/reach.err" & local installing=$!
  for _ in $(seq 120); do grep -q "can't connect" "$root/reach.ndjson" "$root/reach.err" 2> /dev/null && break; kill -0 "$installing" 2> /dev/null || break; sleep 1; done
  grep -q "can't connect.*127.0.0.1:$port didn't answer" "$root/reach.ndjson" "$root/reach.err" 2> /dev/null \
    || check "install failures: a dependency's host that didn't answer wasn't asked again, naming it: $(said reach | tail -c 400)"
  # The host answers the machine again: the next attempt installs.
  sudo iptables -w -D OUTPUT -p tcp -d 127.0.0.1 --dport "$port" -m cgroup --path "$cgroup" -j REJECT --reject-with tcp-reset || true
  code=0; wait "$installing" || code=$?
  [ "$code" = 0 ] || check "install failures: the install didn't complete once its host answered: $(said reach | tail -c 400)"
  kill "$server" 2> /dev/null || true; server=

  # 3. A disk that fills during a dependency's build, its words past uv's kept tail.
  package full "$root/full" $'\n[tool.uv.sources]\ne2e-filler = { path = "filler" }'
  sed -i 's/^dependencies = \[/dependencies = ["e2e-filler", /' "$root/full/pyproject.toml"
  # The filler is the package's own folder, so its source goes to the machine with it.
  mkdir -p "$root/full/filler"
  printf '[build-system]\nrequires = []\nbuild-backend = "backend"\nbackend-path = ["."]\n\n[project]\nname = "e2e-filler"\nversion = "0.1.0"\nrequires-python = ">=3.12"\n' > "$root/full/filler/pyproject.toml"
  cat > "$root/full/filler/backend.py" <<'PY'
"""A build backend whose wheel fills the disk: it takes all but 1 GiB of it at once, writes until ENOSPC, holds the disk full
for 2 s as a build's cleanup would, deletes it all (as uv deletes a partial download), then says 120 KB more, so the disk's
words are past the 64 KiB uv's stderr keeps."""
import os
import sys
import time

NAME, VERSION = "e2e_filler", "0.1.0"


def get_requires_for_build_wheel(config_settings=None):
    return []


def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):
    info = os.path.join(metadata_directory, f"{NAME}-{VERSION}.dist-info")
    os.makedirs(info, exist_ok=True)
    with open(os.path.join(info, "METADATA"), "w") as f:
        f.write(f"Metadata-Version: 2.1\nName: e2e-filler\nVersion: {VERSION}\n")
    return os.path.basename(info)


def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    ballast, path = os.path.join(wheel_directory, "ballast.bin"), os.path.join(wheel_directory, "fill.bin")
    disk = os.statvfs(wheel_directory)
    try:
        with open(ballast, "wb") as f:
            os.posix_fallocate(f.fileno(), 0, max(0, disk.f_bavail * disk.f_frsize - (1 << 30)))
        with open(path, "wb") as f:
            block = os.urandom(1 << 20)
            while True:
                f.write(block)
                f.flush()
    except OSError as error:
        print(f"e2e-filler: {error}", file=sys.stderr, flush=True)
        time.sleep(2)
    finally:
        for made in (path, ballast):
            if os.path.exists(made):
                os.remove(made)
    for line in range(2000):
        print(f"e2e-filler: build log line {line:05d} " + "." * 40, file=sys.stderr)
    raise SystemExit(1)
PY
  uv lock --project "$root/full" > "$root/full-lock.log" 2>&1 || { check "install failures: locking with the filler: $(tail -c 300 "$root/full-lock.log")"; exit 0; }
  install full "$root/full"
  [ "$code" != 0 ] && said full | grep -q machine_disk_full && said full | grep -q "ran out of space while installing the package (uv sync" \
    || check "install failures: a build that filled the disk wasn't machine_disk_full in its sync: $(said full | grep -o '"code": *"[a-z_]*"' | tail -1) $(said full | tail -c 300)"
  echo "install failures: an aarch64 lock refused before its environment; an unanswered host asked again, then installed; a full disk said so"
)
