#!/usr/bin/env bash
# One real GPU cell per invocation, scheduled through the normal default-home Cozy lane.
set -euo pipefail
suite=$(cd "$(dirname "$0")" && pwd)
repo=$(git -C "$suite" rev-parse --show-toplevel)
source "$repo/e2e/run.sh"
mode=${1:?fits, stream, lora or failure}
candidate_python=${2-} # empty means this machine's installed SDK interpreter
out=${3:?unique output directory}
prefetch_on=${4:-local}
mkdir -p "$out"
prefetch_args=(seed=103 mode="$mode")
if [ -n "$candidate_python" ]; then
  prefetch_args+=(candidate_python="$candidate_python")
fi
run_cell "$out" "$mode" "$suite/gpu" "${prefetch_args[@]}" --gpus=1 --on "$prefetch_on"
