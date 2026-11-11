#!/usr/bin/env bash
# The optional filesystem driver on a fresh Ubuntu 24.04 runner, with Ubuntu's listed HTTPS mirror.
# https://packages.ubuntu.com/noble/amd64/libfuse2t64/download
set -euo pipefail
apt_root=${RUNNER_TEMP:?}/cozy-vault-apt
mkdir -p "$apt_root"/{sources,lists/partial,cache/archives/partial}
cat > "$apt_root/noble.list" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble-updates main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.kernel.org/ubuntu noble-security main universe
SOURCES
apt_args=(-o "Dir::Etc::SourceList=$apt_root/noble.list" -o "Dir::Etc::SourceParts=$apt_root/sources"
          -o "Dir::State::Lists=$apt_root/lists" -o "Dir::Cache=$apt_root/cache")
sudo apt-get "${apt_args[@]}" update
sudo apt-get "${apt_args[@]}" install -y fuse libfuse2t64
sudo modprobe fuse || test -e /dev/fuse
