#!/bin/bash
# The fixed benchmark suite in one command (suites/bench/README.md): each group of suites/bench/skus on a fresh pod of its
# own (gate.sh --suite bench; the groups at once), then every group's table against the stored baseline and ComfyUI.
#   bench.sh [--groups group,...] [--out DIR] [--comfyui] [--previous [--between FILE]] [gate.sh's candidate flags: --runtime F --tensorfs F --tensord F ...]
# --previous: each pod first runs the release it boots (the Hub's), held as the candidate's baseline on that same pod.
# --between FILE: run once between the two passes (bash FILE): what lets this computer's Calcifer drive the candidate.
# --comfyui: after a group's cells, ComfyUI on the same pod for the cells with a ComfyUI cell (suites/bench/comfyui.sh; skus
# sizes each pod's disk and hard stop for it), compared in comfyui.md (bench.py versus). Exit 1 on any FAIL. Each pod is
# ended by its gate.sh, which reads it back gone.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/machines.sh"
OUT= WANT= COMFY= PREV= BETWEEN= GATE=()
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --groups) WANT=,$2,; shift 2 ;;
    --comfyui) COMFY=1; shift ;;
    --previous) PREV=1; shift ;;
    --between) BETWEEN=$2; shift 2 ;;
    *) GATE+=("$1"); shift ;;
  esac
done
OUT=${OUT:-$PWD/bench-$(date -u +%Y%m%dT%H%MZ)}
mkdir -p "$OUT"
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a "$OUT/bench.log"; }
GROUPS_RUN=()
while read -r g _ disk cap est _; do
  [ -n "$g" ] && [ "${g:0:1}" != '#' ] || continue
  [ -z "$WANT" ] || [[ $WANT == *",$g,"* ]] || continue
  GROUPS_RUN+=("$g $disk $cap ${est:-0}")
done < "$HERE/suites/bench/skus"
# at <group> <disk> <cap>: gate.sh's arguments for the group's pod.
at() { local base=${1%%.*}; AT=("${base%x*}" --gpus "${base##*x}" --group "$1" --suite bench --disk "$2" --cap "$3"); }
comfy() { [ -n "$COMFY" ] && awk -F'\t' -v g="$1" '$2 == g && $12 != "-" {found = 1} END {exit !found}' "$HERE/suites/bench/cells.tsv"; }
pids=()
if [ -n "$PREV" ]; then
  # Pass 1, the release each pod boots (the Hub's), into previous/<group>; each pod kept, alive, until every group is done.
  # Starts are staggered by skus' estimate so the passes end together. Then --between, once (a candidate of another minor
  # needs this computer's cozy on it: Calcifer sends only to machines on its own minor), then pass 2 on the same pods.
  mkdir -p "$OUT/previous"; rm -f "$OUT/.pass2" "$OUT/.abort"
  most=0; for r in "${GROUPS_RUN[@]}"; do read -r _ _ _ e <<< "$r"; (( e > most )) && most=$e; done
  for r in "${GROUPS_RUN[@]}"; do
    read -r g disk cap e <<< "$r"; at "$g" "$disk" "$cap"
    (
      sleep $(( (most - e) * 60 ))
      timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${AT[@]}" --keep --out "$OUT/previous/$g" > "$OUT/previous/$g.log" 2>&1
      m=$(cat "$OUT/previous/$g/machine" 2>/dev/null)
      grep -qs "kept .* ($m) for the next gate" "$OUT/previous/$g/gate.log" && echo "$m" > "$OUT/previous/$g.kept"
      touch "$OUT/previous/$g.done"
      # Held for pass 2 at most 4 h; then ended, as an idle rental would be, and pass 2 rents its own.
      for ((i = 0; i < 60; i++)); do
        [ -s "$OUT/previous/$g.kept" ] && [ ! -e "$OUT/.pass2" ] && [ ! -e "$OUT/.abort" ] || break
        cozy machine keepalive "$m" > /dev/null 2>&1; sleep 240
      done
      if [ -s "$OUT/previous/$g.kept" ] && { [ -e "$OUT/.abort" ] || [ ! -e "$OUT/.pass2" ]; }; then
        rid=$(cat "$OUT/previous/$g/rental" 2>/dev/null); cozy machine end "${rid:-$m}" -y > "$OUT/previous/$g.end.log" 2>&1; rm -f "$OUT/previous/$g.kept"
      fi
    ) &
    log "$g: pass 1 in $(( most - e )) min, $OUT/previous/$g"
  done
  until n=0; for r in "${GROUPS_RUN[@]}"; do read -r g _ <<< "$r"; [ -e "$OUT/previous/$g.done" ] && n=$((n + 1)); done; (( n == ${#GROUPS_RUN[@]} )); do sleep 30; done
  log "pass 1 done: $(cat "$OUT"/previous/*.kept 2>/dev/null | paste -sd' ') kept"
  if [ -n "$BETWEEN" ] && ! bash "$BETWEEN" >> "$OUT/bench.log" 2>&1; then
    log "--between $BETWEEN failed: the kept pods end, no candidate pass"; touch "$OUT/.abort"; wait; exit 1
  fi
  touch "$OUT/.pass2"
fi
for r in "${GROUPS_RUN[@]}"; do
  read -r g disk cap _ <<< "$r"; at "$g" "$disk" "$cap"
  more=(); comfy "$g" && more=(--cells "$HERE/suites/bench/comfyui.sh")
  on=(); m=$(cat "$OUT/previous/$g.kept" 2>/dev/null) && [ -n "$m" ] && on=(--on "$m" --end)
  [ -z "$PREV" ] || [ ${#on[@]} -gt 0 ] || log "$g: no kept pod: the candidate rents its own, and its previous column is another pod's"
  # The hard stop: gate.sh's TERM trap ends its pod and reads it back gone.
  timeout -k 1800 "$((cap * 60))" "$HERE/gate.sh" "${AT[@]}" "${on[@]}" --out "$OUT/$g" "${more[@]}" "${GATE[@]}" > "$OUT/$g.log" 2>&1 &
  pids+=($!)
  log "$g: ${on[*]:-a fresh pod}, $OUT/$g"
done
for p in "${pids[@]}"; do wait "$p"; done
wait
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
