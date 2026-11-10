#!/bin/bash
# The fixed benchmark suite in one command (suites/bench/README.md): each group of suites/bench/skus on a fresh pod of its
# own (gate.sh --suite bench; the groups at once), then every group's table against the stored baseline and ComfyUI.
#   bench.sh [--groups group,...] [--out DIR] [--comfyui] [gate.sh's candidate flags: --runtime F --tensorfs F --tensord F ...]
# --comfyui: after a group's cells, ComfyUI on the same pod for the cells with a ComfyUI cell (suites/bench/comfyui.sh; ~300 GB
# more disk, up to two hours more), compared in comfyui.md (bench.py versus). Exit 1 on any FAIL. Each pod is ended by its
# gate.sh, which reads it back gone.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/machines.sh"
OUT= WANT= COMFY= GATE=()
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --groups) WANT=,$2,; shift 2 ;;
    --comfyui) COMFY=1; shift ;;
    *) GATE+=("$1"); shift ;;
  esac
done
OUT=${OUT:-$PWD/bench-$(date -u +%Y%m%dT%H%MZ)}
mkdir -p "$OUT"
pids=()
while read -r group _ disk cap _; do
  [ -n "$group" ] && [ "${group:0:1}" != '#' ] || continue
  [ -z "$WANT" ] || [[ $WANT == *",$group,"* ]] || continue
  more=()
  if [ -n "$COMFY" ] && awk -F'\t' -v g="$group" '$2 == g && $12 != "-" {found = 1} END {exit !found}' "$HERE/suites/bench/cells.tsv"; then
    more=(--cells "$HERE/suites/bench/comfyui.sh"); disk=$((disk + 300)) cap=$((cap + 120))
  fi
  # The hard stop: gate.sh's TERM trap ends its pod and reads it back gone.
  timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${group%x*}" --gpus "${group##*x}" --suite bench --disk "$disk" --cap "$cap" --out "$OUT/$group" "${more[@]}" "${GATE[@]}" > "$OUT/$group.log" 2>&1 &
  pids+=($!)
  echo "$group: gate.sh pid $!, $OUT/$group"
done < "$HERE/suites/bench/skus"
for p in "${pids[@]}"; do wait "$p"; done
for r in "$OUT"/*/rental; do
  [ -f "$r" ] || continue
  if listed "$(cat "$r")"; then echo "WARNING: rental $(cat "$r") ($(cat "$(dirname "$r")/machine" 2>/dev/null)) is still listed: end it" >&2
  else echo "$(dirname "$r" | xargs basename): rental $(cat "$r") gone"; fi
done
nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py" versus "$OUT"/*/ > "$OUT/comfyui.md" 2>&1
nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py" table "$OUT"/*/ | tee "$OUT/table.md"
code=${PIPESTATUS[0]}
cat "$OUT/comfyui.md"
exit "$code"
