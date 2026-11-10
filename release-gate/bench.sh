#!/bin/bash
# The fixed benchmark suite in one command (suites/bench/README.md): each group of suites/bench/skus on a fresh pod of its
# own (gate.sh --suite bench; the groups at once), then every group's table against the stored baseline and ComfyUI.
#   bench.sh [--skus FILE] [--groups group,...] [--out DIR] [--comfyui] [gate.sh's candidate flags: --runtime F --tensorfs F --tensord F ...]
# --comfyui: after a group's cells, ComfyUI on the same pod for the cells with a ComfyUI cell (suites/bench/comfyui.sh; skus
# sizes each pod's disk and hard stop for it), compared in comfyui.md (bench.py versus). Exit 1 on any FAIL. Each pod is
# ended by its gate.sh, which reads it back gone.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/machines.sh"
OUT= WANT= COMFY= SKUS=$HERE/suites/bench/skus GATE=()
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --skus) SKUS=$(cd "$(dirname "$2")" && pwd)/$(basename "$2"); shift 2 ;;
    --groups) WANT=,$2,; shift 2 ;;
    --comfyui) COMFY=1; shift ;;
    *) GATE+=("$1"); shift ;;
  esac
done
OUT=${OUT:-$PWD/bench-$(date -u +%Y%m%dT%H%MZ)}
mkdir -p "$OUT"
pids=()
while read -r g _ disk cap _; do
  [ -n "$g" ] && [ "${g:0:1}" != '#' ] || continue
  [ -z "$WANT" ] || [[ $WANT == *",$g,"* ]] || continue
  base=${g%%.*} more=()
  if [ -n "$COMFY" ] && awk -F'\t' -v g="$g" '$12 != "-" && !/^#/ { n = split($2, ps, ","); for (i = 1; i <= n; i++) { p = ps[i]
      if (p == g || (substr(p, 1, 1) == "*" && substr(g, length(g) - length(p) + 2) == substr(p, 2))) found = 1 } } END {exit !found}' "$HERE/suites/bench/cells.tsv"; then
    more=(--cells "$HERE/suites/bench/comfyui.sh")
  fi
  # The hard stop: gate.sh's TERM trap ends its pod and reads it back gone.
  timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${base%x*}" --gpus "${base##*x}" --group "$g" --suite bench --disk "$disk" --cap "$cap" \
    --out "$OUT/$g" "${more[@]}" "${GATE[@]}" > "$OUT/$g.log" 2>&1 &
  pids+=($!)
  echo "$g: gate.sh pid $!, $OUT/$g"
done < "$SKUS"
for p in "${pids[@]}"; do wait "$p"; done
for r in "$OUT"/*/rental; do
  [ -f "$r" ] || continue
  if listed "$(cat "$r")"; then echo "WARNING: rental $(cat "$r") ($(cat "$(dirname "$r")/machine" 2>/dev/null)) is still listed: end it" >&2
  else echo "$(dirname "$r" | xargs basename): rental $(cat "$r") gone"; fi
done
BENCH_PY=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py")
mapfile -t DIRS < <(for d in "$OUT"/*/; do [ ! -f "$d/bench-group" ] || echo "$d"; done)
"${BENCH_PY[@]}" versus "${DIRS[@]}" > "$OUT/comfyui.md" 2>&1
"${BENCH_PY[@]}" table "${DIRS[@]}" | tee "$OUT/table.md"
code=${PIPESTATUS[0]}
cat "$OUT/comfyui.md"
exit "$code"
