#!/usr/bin/env bash
# Browser packages on a fresh Ubuntu 24.04 runner, without changing its APT sources.
set -euo pipefail
pw=$1
apt_root=${RUNNER_TEMP:?}/cozy-browser-apt
mkdir -p "$apt_root"/{sources,lists/partial,cache/archives/partial}
cat > "$apt_root/noble.list" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble-updates main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble-security main universe
SOURCES
cat > "$apt_root/apt.conf" <<CONFIG
Dir::Etc::SourceList "$apt_root/noble.list";
Dir::Etc::SourceParts "$apt_root/sources";
Dir::State::Lists "$apt_root/lists";
Dir::Cache "$apt_root/cache";
CONFIG
# Running only install-deps as root preserves APT_CONFIG through Playwright's apt subprocess.
# The browser download remains the ordinary runner's separate `install firefox` command.
sudo env APT_CONFIG="$apt_root/apt.conf" "$(command -v node)" "$pw/node_modules/playwright-core/cli.js" install-deps firefox
# Mesa's software GL and a virtual display: a runner has no GPU, and Firefox makes no WebGL context (the canvas, cl-991) headless or with no GL.
sudo env APT_CONFIG="$apt_root/apt.conf" apt-get install -y --no-install-recommends libavcodec-extra libegl1 libegl-mesa0 libgl1-mesa-dri libglx-mesa0 libgbm1 xvfb
