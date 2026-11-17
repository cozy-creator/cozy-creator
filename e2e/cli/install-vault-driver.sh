#!/usr/bin/env bash
# The optional filesystem driver on a fresh Ubuntu 24.04 runner, before private fixtures.
#   install-vault-driver.sh             FUSE 2: fusermount and libfuse.so.2 (a released cozy's provider, stuck.sh's fusepy)
#   install-vault-driver.sh fuse3       FUSE 3 beside libfuse.so.2, which the glibc securefs build loads (cl-1000)
#   install-vault-driver.sh fuse3-only  FUSE 3, every FUSE 2 package purged: all the bundled static provider needs (cl-1000)
# Both mirror cohorts require the Ubuntu archive signature and authenticated package hashes.
# Azure's archive also carries security updates (Microsoft's Ubuntu VM sources configuration).
set -euo pipefail
mode=${1:-fuse2}
case $mode in
  fuse2) packages=(fuse libfuse2t64) ;;
  fuse3) packages=(fuse3 libfuse2t64) ;;
  fuse3-only) packages=(fuse3) ;;
  *) echo "install-vault-driver.sh: no mode $mode (fuse2, fuse3, fuse3-only)" >&2; exit 2 ;;
esac
apt_root=${RUNNER_TEMP:?}/cozy-vault-apt
installed=0
for mirror in official azure; do
  attempt_root="$apt_root/$mirror"
  mkdir -p "$attempt_root"/{sources,lists/partial,cache/archives/partial}
  if [ "$mirror" = official ]; then
    archive=https://archive.ubuntu.com/ubuntu
    security=https://security.ubuntu.com/ubuntu
  else
    # The documented Azure transport remains signature-checked; never use trusted=yes,
    # AllowUnauthenticated, an alternate key, or a hardcoded address to bypass a refusal.
    archive=http://azure.archive.ubuntu.com/ubuntu
    security=$archive
  fi
  cat > "$attempt_root/noble.list" <<SOURCES
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] $archive noble main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] $archive noble-updates main universe
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] $security noble-security main universe
SOURCES
  apt_args=(-o "Dir::Etc::SourceList=$attempt_root/noble.list" -o "Dir::Etc::SourceParts=$attempt_root/sources"
            -o "Dir::State::Lists=$attempt_root/lists" -o "Dir::Cache=$attempt_root/cache"
            -o APT::Update::Error-Mode=any -o Acquire::Retries=1 -o Acquire::ForceIPv4=true
            -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15)
  # A partial update must not become a misleading package-not-found error. Each independent
  # mirror has bounded acquisition/installation time and its own signed index directory.
  if sudo timeout --kill-after=10s 90s apt-get "${apt_args[@]}" update &&
     sudo timeout --kill-after=10s 120s apt-get "${apt_args[@]}" install -y "${packages[@]}"; then
    installed=1
    break
  fi
  echo "FUSE driver: signed $mirror repository attempt failed" >&2
done
[ "$installed" = 1 ] || { echo "FUSE driver unavailable: both signed repository attempts failed" >&2; exit 1; }
if [ "$mode" = fuse3-only ]; then
  mapfile -t fuse2 < <(dpkg-query -W -f '${Package} ${db:Status-Status}\n' libfuse2t64 libfuse2 fuse 2> /dev/null | awk '$2 == "installed" { print $1 }')
  [ "${#fuse2[@]}" = 0 ] || sudo timeout --kill-after=10s 120s apt-get remove -y --purge "${fuse2[@]}"
  sudo ldconfig
  left=$(ldconfig -p | grep 'libfuse\.so\.2' || true)
  [ -z "$left" ] || { echo "FUSE 3 alone: libfuse.so.2 is still here: $left" >&2; exit 1; }
fi
[ "$mode" = fuse2 ] || command -v fusermount3 > /dev/null || { echo "FUSE driver: no fusermount3 after installing ${packages[*]}" >&2; exit 1; }
sudo modprobe fuse || test -c /dev/fuse
test -c /dev/fuse
