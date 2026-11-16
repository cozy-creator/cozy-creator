# h3-upscale-pair (cr-297): se-138's failing flow (pod-session 43777750768: H3's no-prompt 1080p Upscale ran out of memory
# in the executor that made the take, 3.15 GB beside 19.2 GiB live) on one rental, head vs slice, through the page route
# (e2e/page.py) as h3-upscale.sh renders it: the gate's Skater request (15 s, seed 2026102801 on head; ...02 on the slice,
# so no arm's render is the other's memo) as a box, so its take keeps its latents. On each arm, after an update to its
# Runtime: the take; its upscale in the executor that made it (`same`); TensorD restarted and the take upscaled again in a
# fresh executor at strength 0.8 (`fresh`: the refine's own size). On the slice, between them, a second take in that
# executor (`again`: its steps once the upscale gave its memory back). The pod reads each process's GPU memory every 5 s
# (gpu-pod.log); each render's record keeps the H3 executor's graphs receipt (pools, idle work). params:
#   head, slice  each a Runtime manylinux wheel: https://…#sha256=<hex>, or a cozy-runtime dev-build run id
#   tensorfs     TensorFS's wheel, https://…#sha256=<hex> or artifact:<cozy-creator run>/<artifact> (both arms)
#   to           1080p (default)
. "$(dirname "$0")/lib.sh"
TO=$(param to 1080p) WAS=$PACKAGES/was/minimax-h3 GATE=$(cd "$(dirname "$0")/.." && pwd)
[ -f "$WAS/pyproject.toml" ] || fail "no packages_refs checkout was"
wheel() {
  local d=$OUT/wheels/$2; mkdir -p "$d"
  if [[ $1 == https://*'#sha256='* ]]; then
    local f; f=$d/$(basename "${1%%#*}" | sed 's/%2B/+/g')
    curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" && [ "$(sha256sum "$f" | cut -c1-64)" = "${1##*sha256=}" ] && echo "$f"
  elif [[ $1 == artifact:*/* ]]; then
    local a=${1#artifact:}
    gh run download "${a%%/*}" -R cozy-creator/cozy-creator -n "${a#*/}" -D "$d" > /dev/null 2>&1 && find "$d" -name '*manylinux*x86_64*.whl' | head -1
  else
    gh run download "$1" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$d" > /dev/null 2>&1 && find "$d" -name '*manylinux*x86_64*.whl' | head -1
  fi
}
declare -A RT=()
for arm in head slice; do RT[$arm]=$(wheel "$(param "$arm")" "$arm") && [ -n "${RT[$arm]}" ] || fail "$arm: no Runtime wheel from $(param "$arm")"; done
TF=$(wheel "$(param tensorfs)" tensorfs) && [ -n "$TF" ] || fail "tensorfs: no wheel"
say "head ${RT[head]##*/}; slice ${RT[slice]##*/}; tensorfs ${TF##*/}"
IFS=$'\t' read -r HERO _ MOTION _ < <(grep -m1 $'^Skater\t' "$GATE/scenarios.tsv")
[ -n "$MOTION" ] || fail "no Skater row in scenarios.tsv"
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
record() { (cd "$RUN_PROJECT" && cozy --json --full run watch "$(newest_run "$1")" --once); }
GIB='/ 1073741824 * 100 | floor / 100'
memory() { jq -c "[.calls[]? | select(.measurements.memory? != null) | .measurements.memory as \$m | {function, status,
  peak_allocated: ((\$m.peak_allocated_bytes // 0) $GIB), driver_before: ([\$m.driver[]?.before_bytes] | max // 0 $GIB),
  driver_peak: ([\$m.driver[]?.peak_bytes] | max // 0 $GIB), varena_allocated: ((\$m.stats.varena[0].stats.allocated // 0) $GIB),
  varena_reserved: ((\$m.stats.varena[0].stats.reserved // 0) $GIB), error: .error.message}]" "$1" 2>&1; }
graphs() { jq -c '[.calls[]? | .measurements.residency.graphs // {} | to_entries[] | select(.value.region_calls // 0 > 0)
  | {c: (.key | split("/") | last), calls: .value.calls, captures: .value.captures, pools: .value.graph_pools,
     pool_bytes: .value.graph_pool_bytes, idle: .value.idle, why: .value.why}]' "$1" 2>&1; }
steps() { jq -c '[.calls[]? | .measurements.attribution.steps.denoise.series // empty | map(.[1] / 1000 | . * 100 | floor / 100)]' "$1" 2>&1; }
# render <box> <tool> <fields json>: the box added and rendered; its record in $OUT/<box>.json. A failed render is said.
render() {
  run_page add "$1" --tool "$2" --fields "$3" > "$OUT/add-$1.txt" 2>&1 || fail "add $1: $(cat "$OUT/add-$1.txt")"
  echo "$1" > "$OUT/current"
  say "$1: start"
  local ok=0 t; t=$(date +%s); run_page render --on "$NAME" "$1" > "$OUT/render-$1.txt" 2>&1 || ok=1
  record "$1" > "$OUT/$1.json" 2>&1
  say "$1: $([ "$ok" = 0 ] && echo done || echo FAILED) in $(( $(date +%s) - t )) s, run $(newest_run "$1"), memoized $(jq -r '[.calls[]?.memoized] | any' "$OUT/$1.json")"
  [ "$ok" = 0 ] || say "$1 failure: $(grep -o '"failure": {[^}]*}' "$OUT/render-$1.txt" | tail -1 | cut -c1-600)"
  say "$1 memory: $(memory "$OUT/$1.json")"
  say "$1 graphs: $(graphs "$OUT/$1.json" | cut -c1-1500)"
  say "$1 steps: $(steps "$OUT/$1.json")"
  return "$ok"
}
take() { render "$1" "$WAS/generate" "$(jq -cn --arg d "<$HERO> $MOTION" --argjson s "$2" '{seed: $s, beats: [{duration: 15, description: $d}]}')"; }
upscale() { render "$1" "$WAS/upscale" "$(jq -cn --arg v "<$2>" --arg to "$TO" --argjson x "${3:-null}" '{video: $v, to: $to} + (if $x then {strength: $x} else {} end)')"; }

kernels "$WAS"
cp "$REF/seed.jpg" "$OUT/skater.jpg"
run_page import "$OUT/skater.jpg" --name "$HERO" > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
RENT_ARGS=--ssh  # the pod's root shell reads its GPU (gpu-reader.sh): the runner needs a key
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
quote "$(param need_s 6000)"
rent
. "$(dirname "$0")/gpu-reader.sh"
fails=0
for arm in head slice; do
  step "update-$arm" machine update "$NAME" --tensord "$(command -v cozy)" --runtime "${RT[$arm]}" --tensorfs "$TF" -y || fail "update $arm"
  say "$arm: pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs}')"
  gpuwatch
  seed=2026102801; [ "$arm" = head ] || seed=2026102802
  take "take_$arm" "$seed" || { fails=$((fails + 1)); continue; }
  readings
  upscale "same_$arm" "take_$arm" || fails=$((fails + 1))
  readings
  if [ "$arm" = slice ]; then take "again_$arm" 2026102803 || fails=$((fails + 1)); fi
  step "fresh-$arm" machine update "$NAME" --gpu-memory-limit off -y || fail "restart $arm"
  gpuwatch
  upscale "fresh_$arm" "take_$arm" 0.8 || fails=$((fails + 1))
  readings
done
rm -f "$OUT/current"
[ "$fails" = 0 ] || fail "$fails renders failed"
