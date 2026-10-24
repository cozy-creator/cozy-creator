#!/usr/bin/env bash
# One real GPU cell per invocation, scheduled through the normal default-home Cozy lane.
set -euo pipefail
suite=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$suite" rev-parse --show-toplevel)
source "$repo/e2e/run.sh"
mode=${1:?fits, stream, lora or failure}
candidate_python=${2:?absolute candidate Python path}
out=${3:?unique output directory}
mkdir -p "$out"
run_cell "$out" "$mode" "$suite/gpu" candidate_python="$candidate_python" \
  seed=103 mode="$mode" --gpus=1 --on local
