# streamed-native: one RTX 4090 rental for packages #662 (Qwen streamed denoise) and #661 (SDXL phase shared scratch):
#   1. the production cohort (cozy 0.13.1, Runtime 0.34.2, TensorFS 0.17.2);
#   2. the reduced-model cells: qwen-streamed (14), sdxl-phases (3), sdxl-pressure (3), same-PID where declared;
#   3. under --gpu-memory-limit QCAP: Qwen-Image-2 1 MP / 40 steps, packages master (eager) vs #662, cold then warm;
#   4. under --gpu-memory-limit SCAP: SDXL 1024² / 30 steps, packages master vs #661, cold then warm.
# Each full run records its time, stages, bytes moved to the GPU and the request_path metrics.
# packages_refs: m=<packages master> q=<#662 head> s=<#661 head>. params: qcap (10GiB), scap (6GiB), cells (true),
#   traffic_gb (60), need_s (3600)
. "$(dirname "$0")/lib.sh"
CREATOR=$(cd "$(dirname "$0")/../.." && pwd) X=$CREATOR/release-gate/suites/execution
QCAP=$(param qcap 10GiB) SCAP=$(param scap 6GiB) CELLS=$(param cells true)
S=$(( $(date +%s) % 100000000 * 10 ))
reports() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '[.status.version, .status.runtime, .status.tensorfs] | join(" ")'; }
show() {
  local N; N=$(runno "$1"); [ -n "$N" ] || return 1
  cozy run show "$N" --once --json > "$OUT/$1.json" 2>/dev/null
  jq -e --argjson n "$N" '.number == $n' "$OUT/$1.json" >/dev/null 2>&1
}
# facts <name>: one line: status, pid, execution, stages, bytes moved, request_path metrics, driver peak.
facts() {
  jq -c '{status, run: .number, pid: (.measurements.execution.gpus[0].pid // null), exec_ms: .execution_ms,
    stages: ((.measurements.attribution // {}) | with_entries(select(.value | type == "object" and has("total_ms"))) | map_values(.total_ms | floor)),
    moved: ([.measurements.memory.stats.varena[]?.stats.moved // {} | to_entries[] | .value.bytes] | add),
    metrics: ([.measurements.observations[]? | select(.name | startswith("request_path") or startswith("release_gate")) | {(.name): .value}] | add),
    driver_peak: ([.measurements.memory.driver[]?.peak_bytes] | max), software: .measurements.software,
    reason: (.reason // null | if . then {code, message: (.message // "" | .[0:300])} else null end)}' "$OUT/$1.json" 2>/dev/null
}
passed() { jq -e '.status == "completed" and (.result.passed // true)' "$OUT/$1.json" >/dev/null 2>&1; }
pid_of() { jq -r '.measurements.execution.gpus[0].pid // empty' "$OUT/$1.json" 2>/dev/null; }
# cell <name> <same-pid as previous: true|false> <target> <args…>: one fixture run, judged by its record.
PREV=''
cell() {
  local n=$1 same=$2 p; shift 2
  step "$n" run "$@" --on "$NAME" --gpus=1; show "$n" || { say "$n: no run record"; RC=1; PREV=''; return; }
  p=$(pid_of "$n")
  if passed "$n" && { [ "$same" != true ] || [ "$p" = "$PREV" ]; }; then say "$n: PASS pid $p"; else
    say "$n: FAIL pid $p (previous $PREV, same=$same): $(facts "$n" | cut -c1-600)"; RC=1; fi
  PREV=$p
}
# full <name> <tree> <target> <args…>: one full-model run from a packages checkout, its facts logged.
full() {
  local n=$1 tree=$2; shift 2
  cd "$PACKAGES/$tree" || fail "no packages/$tree"
  step "$n" run "$@" --on "$NAME" --out "$OUT/out/$n"; show "$n"
  cd "$OUT" || fail "no $OUT"
  say "$n ($tree $(git -C "$PACKAGES/$tree" rev-parse --short HEAD)): $(facts "$n")"
  passed "$n" || RC=1
}
cap() {
  step "cap-$1" machine update "$NAME" --gpu-memory-limit "$1" -y || fail "cap $1: machine update failed"
  say "cap $1: $(cozy machine show "$NAME" 2>/dev/null | grep -iE 'limit|GPU 0' | tr -s ' ' | paste -sd';')"
}
collect() {
  [ -n "$NAME" ] || return 0
  timeout 120 cozy machine logs "$NAME" tensord -n 0 > "$OUT/machine-tensord.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/machine-memory.log" 2>&1
}
trap 's=$?; collect; (exit $s); finish' EXIT

# $0 checks: the three fixtures, the three packages trees, the parsers.
if [ "$(git -C "$CREATOR" config --get core.sparseCheckout)" = true ]; then
  git -C "$CREATOR" sparse-checkout add release-gate/suites/execution/qwen-streamed release-gate/suites/execution/sdxl-phases \
    release-gate/suites/execution/sdxl-pressure || fail "fixtures not checked out"
fi
for f in qwen-streamed/ci_qwen_streamed/_prepared.py sdxl-phases/ci_sdxl_phases/_prepared.py sdxl-pressure/ci_sdxl_phases/_prepared.py; do
  [ -s "$X/$f" ] || fail "no $X/$f"; done
cmp -s "$X/qwen-streamed/ci_qwen_streamed/_prepared.py" "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "qwen fixture is not q's _prepared.py"
for d in sdxl-phases sdxl-pressure; do for f in _prepared.py _streamed.py; do
  cmp -s "$X/$d/ci_sdxl_phases/$f" "$PACKAGES/s/sdxl/sdxl/$f" || fail "$d/$f is not s's"; done; done
[ -s "$PACKAGES/m/qwen-image-2/qwen_image_2/model.py" ] && [ -s "$PACKAGES/m/sdxl/sdxl/__init__.py" ] || fail "no master tree"
say "trees: creator $(git -C "$CREATOR" rev-parse --short HEAD), m $(git -C "$PACKAGES/m" rev-parse --short HEAD), q $(git -C "$PACKAGES/q" rev-parse --short HEAD), s $(git -C "$PACKAGES/s" rev-parse --short HEAD)"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
echo '{"number":3,"status":"completed","execution_ms":5,"result":{"passed":true},"measurements":{"execution":{"gpus":[{"pid":9}]},"memory":{"stats":{"varena":[{"stats":{"moved":{"store":{"bytes":7}}}}]}},"observations":[{"name":"request_path.x","value":1}]}}' > "$OUT/probe.json"
[ "$(facts probe | jq -r '.moved')" = 7 ] && [ "$(pid_of probe)" = 9 ] && passed probe || fail "judges"
quote "$(param traffic_gb 60)" "$(param need_s 3600)"
rent

# 1: the production cohort.
want="0.13.1 0.34.2 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.2 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
say "cohort: $NAME runs $want"

# 2: reduced-model cells.
if [ "$CELLS" = true ]; then
  i=0
  while IFS=$'\t' read -r n lane mode same; do
    cell "q-$n" "$same" "$X/qwen-streamed/gpu" "lane=$lane" "mode=$mode" "seed:=$((S + 100 + i))"; i=$((i + 1))
  done < <(printf '%s\n' \
    'bf16-repeat-0	bf16	repeat	false' 'bf16-reuse-1	bf16	reuse	false' 'bf16-pressure-2	bf16	pressure	false' \
    'bf16-identity-3	bf16	identity	false' 'bf16-cancel-4	bf16	cancel	false' 'bf16-repeat-5	bf16	repeat	true' \
    'int8-repeat-6	int8	repeat	false' 'int8-reuse-7	int8	reuse	false' 'int8-cancel-8	int8	cancel	false' \
    'int8-repeat-9	int8	repeat	true' 'bf16-selection	bf16	selection	false' 'int8-selection	int8	selection	false' \
    'bf16-not-local	bf16	not-local	false' 'bf16-after-not-local	bf16	selection	true')
  cell s-phases-0 false "$X/sdxl-phases/gpu" mode=phases steps:=30 guidance:=7.0 "seed:=$((S + 200))"
  cell s-refusal-1 true "$X/sdxl-phases/gpu" mode=prepare-refusal steps:=2 guidance:=7.0 "seed:=$((S + 201))"
  cell s-phases-2 true "$X/sdxl-phases/gpu" mode=phases steps:=2 guidance:=7.0 "seed:=$((S + 202))"
  cell p-build false "$X/sdxl-pressure/gpu" mode=build steps:=2 guidance:=7.0 "seed:=$((S + 300))"
  cell p-pressure true "$X/sdxl-pressure/gpu" mode=pressure steps:=2 guidance:=7.0 "seed:=$((S + 301))"
  cell p-recover true "$X/sdxl-pressure/gpu" mode=recover steps:=2 guidance:=7.0 "seed:=$((S + 302))"
fi

# 3: Qwen-Image-2 under QCAP: master (eager) then #662, cold (same request) then warm (fresh requests).
cap "$QCAP"
QP="a lighthouse on a basalt cliff at blue hour, waves breaking below, long exposure photograph"
full qe-cold m ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 401))" megapixels:=1
full qn-cold q ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 401))" megapixels:=1
full qe-warm m ./qwen-image-2/generate "prompt=a bowl of ramen with a soft egg, steam rising, overhead food photograph" "seed:=$((S + 402))" megapixels:=1
full qn-warm q ./qwen-image-2/generate "prompt=a red vintage bicycle leaning on a yellow wall, afternoon sun" "seed:=$((S + 403))" megapixels:=1

# 4: SDXL under SCAP: master then #661.
cap "$SCAP"
SP="a red fox resting beneath snow-covered pine branches at dawn, detailed natural photograph"
full se-cold m ./sdxl/generate "prompt=$SP" "seed:=$((S + 501))" steps:=30
full sn-cold s ./sdxl/generate "prompt=$SP" "seed:=$((S + 501))" steps:=30
full se-warm m ./sdxl/generate "prompt=a paper boat on a rain puddle, city reflections, macro photograph" "seed:=$((S + 502))" steps:=30
full sn-warm s ./sdxl/generate "prompt=a copper teapot on a wooden shelf, window light, still life" "seed:=$((S + 503))" steps:=30
for f in "$OUT"/out/*/*; do [ -f "$f" ] && say "output $(file -b "$f" | cut -c1-60) $(sha256sum "$f" | cut -c1-16) ${f#$OUT/}"; done
[ "$RC" = 0 ] || fail "see results.txt"
