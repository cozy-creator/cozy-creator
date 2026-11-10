#!/usr/bin/env bash
# One package's environment, as TensorD builds it (tensord/src/published/environment.rs), with CPU torch.
#   env.sh <package dir> <env dir> <runtime dir|wheel> <tensorfs wheel>
# The locked base is kept while the lock is unchanged; the candidates and the package go over it every time.
set -euo pipefail
pkg=$1 env=$2 runtime=$3 tensorfs=$4 py=$2/bin/python
UV=$(command -v uv)
uv() { nice -n 19 "$UV" "$@"; }
key=$(cat "$pkg/uv.lock" "$pkg/pyproject.toml" "$0" | sha256sum | cut -c1-16)
if [ "$(cat "$env/.base" 2> /dev/null)" != "$key" ]; then
  rm -rf "$env"
  uv venv --no-project --python 3.12 "$env" -q
  uv pip install -q --python "$py" --torch-backend cpu torch torchvision torchaudio
  UV_PROJECT_ENVIRONMENT=$env uv sync -q --frozen --inexact --no-dev --no-install-project \
    --no-install-package cozy-runtime --no-install-package tensorfs --project "$pkg" --python "$py"
  echo "$key" > "$env/.base"
fi
uv pip install -q --python "$py" --no-deps --reinstall "$tensorfs"
if [ -d "$runtime" ]; then uv pip install -q --python "$py" --no-deps -e "$runtime"
else uv pip install -q --python "$py" --no-deps --reinstall "$runtime"; fi
uv pip install -q --python "$py" --no-deps --no-sources -e "$pkg"
