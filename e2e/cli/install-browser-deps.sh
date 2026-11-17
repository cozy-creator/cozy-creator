#!/usr/bin/env bash
# Browser packages on a fresh Ubuntu 24.04 runner, without changing its APT sources.
set -euo pipefail
pw=$1
apt_root=${RUNNER_TEMP:?}/cozy-browser-apt
mkdir -p "$apt_root"/{sources,lists/partial,cache/archives/partial}
cat > "$apt_root/apt.conf" <<CONFIG
Dir::Etc::SourceList "$apt_root/noble.list";
Dir::Etc::SourceParts "$apt_root/sources";
Dir::State::Lists "$apt_root/lists";
Dir::Cache "$apt_root/cache";
Acquire::ForceIPv4 "true";
Acquire::Retries "5";
CONFIG
# IPv4 alone: a hosted runner has no IPv6 route, and apt fetched from mirrors.kernel.org's mirror by its AAAA address and gave
# up on "Network is unreachable" (44041520735, 43997435909); every fetch, the lists' and install-deps' alike, retried.
as_apt() { sudo env APT_CONFIG="$apt_root/apt.conf" "$@"; }

# The package lists, from the first mirror that answers: each tried 5 times with backoff, said as it goes. apt-get update
# exits 0 when a list fails to download, so its "Failed to fetch" is what counts.
mirrors=(https://mirrors.kernel.org/ubuntu http://archive.ubuntu.com/ubuntu)
listed=
for mirror in "${mirrors[@]}"; do
  printf 'deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] %s %s main universe\n' \
    "$mirror" noble "$mirror" noble-updates "$mirror" noble-security > "$apt_root/noble.list"
  for attempt in 1 2 3 4 5; do
    [ "$attempt" = 1 ] || { echo "apt-get update: attempt $attempt of 5 on ${mirror#*//} in $((2 ** (attempt - 1))) s" >&2; sleep $((2 ** (attempt - 1))); }
    if as_apt apt-get update > "$apt_root/update.log" 2>&1 && ! grep -qE '^(W: Failed to fetch|E: )' "$apt_root/update.log"; then
      listed=$mirror; break 2
    fi
    grep -E '^(W: Failed to fetch|E: )' "$apt_root/update.log" | head -1 >&2 || true
  done
done
[ -n "$listed" ] || { echo "apt mirror unreachable: ${mirrors[*]#*//}, 5 attempts each (the runner's network, not a cell)" >&2; exit 1; }

# Running only install-deps as root preserves APT_CONFIG through Playwright's apt subprocess.
# The browser download remains the ordinary runner's separate `install firefox` command.
as_apt "$(command -v node)" "$pw/node_modules/playwright-core/cli.js" install-deps firefox
# Mesa's software GL and a virtual display: a runner has no GPU, and Firefox makes no WebGL context (the canvas, cl-991) headless or with no GL.
as_apt apt-get install -y --no-install-recommends libavcodec-extra libegl1 libegl-mesa0 libgl1-mesa-dri libglx-mesa0 libgbm1 xvfb
