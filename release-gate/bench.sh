#!/bin/bash
# The fixed benchmark suite in one command (suites/bench/README.md): each group of suites/bench/skus on a fresh pod of its
# own (gate.sh --suite bench; the groups at once), then every group's table against the stored baseline and ComfyUI.
#   bench.sh [--groups group,...] [--out DIR] [--comfyui] [--previous] [gate.sh's candidate flags: --runtime F --tensorfs F --tensord F ...]
# --previous: each pod first runs the release it boots (the Hub's), held as the candidate's baseline on that same pod.
# --comfyui: after a group's cells, ComfyUI on the same pod for the cells with a ComfyUI cell (suites/bench/comfyui.sh; skus
# sizes each pod's disk and hard stop for it), compared in comfyui.md (bench.py versus). Exit 1 on any FAIL. Each pod is
# ended by its gate.sh, which reads it back gone.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/machines.sh"
OUT= WANT= COMFY= PREV= GATE=()
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --groups) WANT=,$2,; shift 2 ;;
    --comfyui) COMFY=1; shift ;;
    --previous) PREV=1; shift ;;
    *) GATE+=("$1"); shift ;;
  esac
done
OUT=${OUT:-$PWD/bench-$(date -u +%Y%m%dT%H%MZ)}
mkdir -p "$OUT"
[ -z "$PREV" ] || mkdir -p "$OUT/previous"
# group <name> <disk> <cap> <more...>: the group's gates, one after the other on one pod. With --previous, first the release the
# pod boots (the Hub's), its cells into previous/<group>, the pod kept; then the candidate on that same pod, then ComfyUI.
group() {
  local g=$1 disk=$2 cap=$3 on=() m base=${1%%.*}; shift 3
  local at=("${base%x*}" --gpus "${base##*x}" --group "$g" --suite bench --disk "$disk" --cap "$cap")
  if [ -n "$PREV" ]; then
    # The hard stop: gate.sh's TERM trap ends its pod (kept or not) and reads it back gone.
    timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${at[@]}" --keep --out "$OUT/previous/$g" > "$OUT/previous/$g.log" 2>&1
    m=$(cat "$OUT/previous/$g/machine" 2>/dev/null)
    if [ -n "$m" ] && grep -q "kept .* ($m) for the next gate" "$OUT/previous/$g/gate.log"; then on=(--on "$m" --end)
    else echo "$g: no kept pod from the previous release's gate: the candidate rents its own, and its previous column is another pod's"; fi
  fi
  timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${at[@]}" "${on[@]}" --out "$OUT/$g" "$@" "${GATE[@]}" > "$OUT/$g.log" 2>&1
}
pids=()
while read -r g _ disk cap _; do
  [ -n "$g" ] && [ "${g:0:1}" != '#' ] || continue
  [ -z "$WANT" ] || [[ $WANT == *",$g,"* ]] || continue
  more=()
  if [ -n "$COMFY" ] && awk -F'\t' -v g="$g" '$2 == g && $12 != "-" {found = 1} END {exit !found}' "$HERE/suites/bench/cells.tsv"; then
    more=(--cells "$HERE/suites/bench/comfyui.sh")
  fi
  group "$g" "$disk" "$cap" "${more[@]}" &
  pids+=($!)
  echo "$g: pid $!, $OUT/$g"
done < "$HERE/suites/bench/skus"
for p in "${pids[@]}"; do wait "$p"; done
for r in "$OUT"/*/rental "$OUT"/previous/*/rental; do
  [ -f "$r" ] || continue
  if listed "$(cat "$r")"; then echo "WARNING: rental $(cat "$r") ($(cat "$(dirname "$r")/machine" 2>/dev/null)) is still listed: end it" >&2
  else echo "$(dirname "$r" | xargs basename): rental $(cat "$r") gone"; fi
done
BENCH_PY=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py")
gates() { for d in "$1"/*/; do [ ! -f "$d/bench-group" ] || echo "$d"; done; }  # a group's gate dirs, not previous/ or refs/
mapfile -t DIRS < <(gates "$OUT")
judge=() vs=()
if [ -n "$PREV" ]; then
  # The previous release's cells, on the same pods, as the baseline the candidate is held to; its reference outputs stay here.
  mapfile -t PDIRS < <(gates "$OUT/previous")
  "${BENCH_PY[@]}" baseline "$(cat "$OUT"/previous/*/label 2>/dev/null | head -1)" "${PDIRS[@]}" --out "$OUT/previous.json" --refs "$OUT/refs" > /dev/null
  judge=(--baseline "$OUT/previous.json" --refs "$OUT/refs") vs=(--previous "$OUT/previous")
fi
"${BENCH_PY[@]}" versus "${DIRS[@]}" "${vs[@]}" > "$OUT/comfyui.md" 2>&1
"${BENCH_PY[@]}" table "${DIRS[@]}" "${judge[@]}" | tee "$OUT/table.md"
code=${PIPESTATUS[0]}
cat "$OUT/comfyui.md"
exit "$code"
