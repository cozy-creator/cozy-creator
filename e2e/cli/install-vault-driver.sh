#!/usr/bin/env bash
# The optional filesystem driver on a fresh Ubuntu 24.04 runner, without its failing Azure mirror.
set -euo pipefail
apt_root=${RUNNER_TEMP:?}/cozy-vault-apt
mkdir -p "$apt_root"/{sources,lists/partial,cache/archives/partial}
cat > "$apt_root/noble.list" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu noble main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu noble-updates main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://security.ubuntu.com/ubuntu noble-security main universe
SOURCES
apt_args=(-o "Dir::Etc::SourceList=$apt_root/noble.list" -o "Dir::Etc::SourceParts=$apt_root/sources"
          -o "Dir::State::Lists=$apt_root/lists" -o "Dir::Cache=$apt_root/cache")
sudo apt-get "${apt_args[@]}" update
sudo apt-get "${apt_args[@]}" install -y fuse libfuse2t64
sudo modprobe fuse || test -e /dev/fuse
