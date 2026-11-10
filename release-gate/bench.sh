#!/bin/bash
# The fixed benchmark suite in one command (suites/bench/README.md): each group of suites/bench/skus on a fresh pod of its
# own (gate.sh --suite bench; the groups at once), then every group's table against the stored baseline and ComfyUI.
#   bench.sh [--groups group,...] [--out DIR] [gate.sh's candidate flags: --runtime F --tensorfs F --tensord F --packages DIR ...]
# Exit 1 on any FAIL. Each pod is ended by its gate.sh, which reads it back gone.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT= WANT= GATE=()
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --groups) WANT=,$2,; shift 2 ;;
    *) GATE+=("$1"); shift ;;
  esac
done
OUT=${OUT:-$PWD/bench-$(date -u +%Y%m%dT%H%MZ)}
mkdir -p "$OUT"
pids=()
while read -r group _ disk _; do
  [ -n "$group" ] && [ "${group:0:1}" != '#' ] || continue
  [ -z "$WANT" ] || [[ $WANT == *",$group,"* ]] || continue
  "$HERE/gate.sh" "${group%x*}" --gpus "${group##*x}" --suite bench --disk "$disk" --out "$OUT/$group" "${GATE[@]}" > "$OUT/$group.log" 2>&1 &
  pids+=($!)
  echo "$group: gate.sh pid $!, $OUT/$group"
done < "$HERE/suites/bench/skus"
for p in "${pids[@]}"; do wait "$p"; done
nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py" table "$OUT"/*/ | tee "$OUT/table.md"
exit "${PIPESTATUS[0]}"
