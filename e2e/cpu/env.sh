#!/usr/bin/env bash
# One package's environment, as TensorD builds it (tensord/src/published/environment.rs), with CPU torch.
#   env.sh <package dir> <env dir> <runtime dir|wheel> <tensorfs wheel>
set -euo pipefail
pkg=$1 env=$2 runtime=$3 tensorfs=$4 py=$2/bin/python
nice -n 19 uv venv --no-project --python 3.12 "$env" -q
nice -n 19 uv pip install -q --python "$py" --torch-backend cpu torch torchvision torchaudio
UV_PROJECT_ENVIRONMENT=$env nice -n 19 uv sync -q --frozen --inexact --no-dev --no-install-project \
  --no-install-package cozy-runtime --no-install-package tensorfs --project "$pkg" --python "$py"
nice -n 19 uv pip install -q --python "$py" --no-deps "$tensorfs"
if [ -d "$runtime" ]; then nice -n 19 uv pip install -q --python "$py" --no-deps -e "$runtime"
else nice -n 19 uv pip install -q --python "$py" --no-deps "$runtime"; fi
nice -n 19 uv pip install -q --python "$py" --no-deps --no-sources -e "$pkg"
