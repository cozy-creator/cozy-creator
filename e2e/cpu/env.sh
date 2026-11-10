#!/usr/bin/env bash
# One package's environment, as TensorD builds it (tensord/src/published/environment.rs), with CPU torch.
#   env.sh <package dir> <env dir> <runtime> <tensorfs> [helper dir...]
# The Runtime and TensorFS are a wheel, a source dir (Runtime), an exact release version, or `lock`: what the package's
# own lock pins (a released package as it was published). The locked base is kept while the lock is unchanged; the
# candidates and the package go over it every time. A helper dir replaces its locked release (a helper changed beside
# the package, not yet published).
set -euo pipefail
pkg=$1 env=$2 runtime=$3 tensorfs=$4 py=$2/bin/python
shift 4
UV=$(command -v uv)
uv() { nice -n 19 "$UV" "$@"; }
skip=()
[ "$runtime" = lock ] || skip+=(--no-install-package cozy-runtime)
[ "$tensorfs" = lock ] || skip+=(--no-install-package tensorfs)
for helper in "$@"; do skip+=(--no-install-package "$(basename "$helper")"); done
key=$( (cat "$pkg/uv.lock" "$pkg/pyproject.toml" "$0"; echo "${skip[*]}") | sha256sum | cut -c1-16)
if [ "$(cat "$env/.base" 2> /dev/null)" != "$key" ]; then
  rm -rf "$env"
  uv venv --no-project --python 3.12 "$env" -q
  uv pip install -q --python "$py" --torch-backend cpu torch torchvision torchaudio
  UV_PROJECT_ENVIRONMENT=$env uv sync -q --frozen --inexact --no-dev --no-install-project \
    "${skip[@]}" --project "$pkg" --python "$py"
  echo "$key" > "$env/.base"
fi
# A candidate: a wheel or source dir as it is, a version from PyPI.
candidate() {
  if [ -d "$2" ]; then uv pip install -q --python "$py" --no-deps -e "$2"
  elif [ -f "$2" ]; then uv pip install -q --python "$py" --no-deps --reinstall "$2"
  elif [ "$2" != lock ]; then uv pip install -q --python "$py" --no-deps --reinstall "$1==$2"; fi
}
candidate tensorfs "$tensorfs"
candidate cozy-runtime "$runtime"
uv pip install -q --python "$py" --no-deps --no-sources -e "$pkg"
for helper in "$@"; do uv pip install -q --python "$py" --no-deps --no-sources -e "$helper"; done
