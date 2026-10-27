# streamed-native-3060: one RTX 3060 (12 GB) rental, where Qwen-Image-2's int8 transformer cannot stay resident:
#   1. production cozy 0.13.1 / Runtime 0.34.2 / TensorFS 0.17.2; the 14 reduced qwen-streamed cells (#662);
#   2. fetch gating baseline on production TensorD: a fresh Qwen-Image-2 fetch (packages #657's tree, 1 MP, its WebP
#      output) with a weightless ready GPU request submitted behind it;
#   3. Qwen-Image-2 1 MP / 40 steps: packages master (eager) vs #662, cold then warm;
#   4. TensorD from creator #2154 (the runner's dev cozy): the same ready request behind a fresh SDXL fetch;
#   5. SDXL 1024² / 30 steps: packages master vs #661, cold then warm.
# packages_refs: m=<master> q=<#662> s=<#661> w=<#657>. params: cells (true), traffic_gb (60), need_s (3600)
. "$(dirname "$0")/lib.sh"
CREATOR=$(cd "$(dirname "$0")/../.." && pwd) X=$CREATOR/release-gate/suites/execution
CELLS=$(param cells true)
S=$(( $(date +%s) % 100000000 * 10 ))
reports() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '[.status.version, .status.runtime, .status.tensorfs] | join(" ")'; }
show() {
  local N; N=$(runno "$1"); [ -n "$N" ] || return 1
  cozy run show "$N" --once --json > "$OUT/$1.json" 2>/dev/null
  jq -e --argjson n "$N" '.number == $n' "$OUT/$1.json" >/dev/null 2>&1
}
facts() {
  jq -c '{status, run: .number, pid: (.measurements.execution.gpus[0].pid // null), exec_ms: .execution_ms,
    accepted: .accepted_at_ms, started: .started_at_ms, settled: .settled_at_ms,
    stages: ((.measurements.attribution.stages // .measurements.attribution // {}) | with_entries(select(.value | type == "object" and has("total_ms"))) | map_values({ms: (.total_ms | floor), at: .started_unix_ms})),
    moved: ([.measurements.memory.stats.varena[]?.stats.moved // {} | to_entries[] | .value.bytes] | add),
    metrics: ([.measurements.observations[]? | select(.name | startswith("request_path") or startswith("release_gate")) | {(.name): .value}] | add),
    driver_peak: ([.measurements.memory.driver[]?.peak_bytes] | max), software: .measurements.software,
    reason: (.reason // null | if . then {code, message: (.message // "" | .[0:300])} else null end)}' "$OUT/$1.json" 2>/dev/null
}
passed() { jq -e '.status == "completed" and (.result.passed // true)' "$OUT/$1.json" >/dev/null 2>&1; }
pid_of() { jq -r '.measurements.execution.gpus[0].pid // empty' "$OUT/$1.json" 2>/dev/null; }
PREV=''
cell() {
  local n=$1 same=$2 p; shift 2
  step "$n" run "$@" --on "$NAME" --gpus=1; show "$n" || { say "$n: no run record"; RC=1; PREV=''; return; }
  p=$(pid_of "$n")
  if passed "$n" && { [ "$same" != true ] || [ "$p" = "$PREV" ]; }; then say "$n: PASS pid $p"; else
    say "$n: FAIL pid $p (previous $PREV, same=$same): $(facts "$n" | cut -c1-500) $(jq -r '.reason.detail // ""' "$OUT/$n.json" | grep -E 'Error|assert' | tail -2 | cut -c1-300)"; RC=1; fi
  PREV=$p
}
full() {
  local n=$1 tree=$2; shift 2
  cd "$PACKAGES/$tree" || fail "no packages/$tree"
  step "$n" run "$@" --on "$NAME" --out "$OUT/out/$n"; show "$n"
  cd "$OUT" || fail "no $OUT"
  say "$n ($tree $(git -C "$PACKAGES/$tree" rev-parse --short HEAD)): $(facts "$n")"
  passed "$n" || RC=1
}
# gated <label> <tree> <target> <args…>: the target from a packages tree in the background (its weights fetching
# on a fresh machine), then the warm ready request once that run is accepted; both records kept.
gated() {
  local l=$1 tree=$2 p N=''; shift 2
  cd "$PACKAGES/$tree" || fail "no packages/$tree"
  cozy run "$@" --on "$NAME" --out "$OUT/out/$l-w" > "$OUT/$l-w.txt" 2>&1 & p=$!
  cd "$OUT" || fail "no $OUT"
  for _ in $(seq 120); do N=$(runno "$l-w"); [ -n "$N" ] && break; sleep 1; done
  [ -n "$N" ] || { kill "$p"; fail "$l: the fetching run never got a number"; }
  sleep 3
  step "$l-r" run "$X/gpu-ready/gpu" "tag=$l" --on "$NAME" --gpus=1; show "$l-r"
  wait "$p"; show "$l-w"
  say "$l fetching run: $(facts "$l-w")"
  say "$l ready run: $(facts "$l-r") entered_ms $(jq -r '.result.entered_ms // empty' "$OUT/$l-r.json")"
  local first; first=$(jq -r '[.measurements.attribution.stages // .measurements.attribution // {} | to_entries[] | .value.started_unix_ms? // empty] | min // empty' "$OUT/$l-w.json")
  say "$l verdict: ready settled $(jq -r .settled_at_ms "$OUT/$l-r.json"), fetching run's first stage $first: $( [ -n "$first" ] && [ "$(jq -r .settled_at_ms "$OUT/$l-r.json")" -lt "$first" ] && echo 'ready ran while the inputs fetched' || echo 'ready waited behind the fetching run')"
}
collect() {
  [ -n "$NAME" ] || return 0
  timeout 120 cozy machine logs "$NAME" tensord -n 0 > "$OUT/machine-tensord.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/machine-memory.log" 2>&1
}
trap 's=$?; collect; (exit $s); finish' EXIT

# $0 checks.
if [ "$(git -C "$CREATOR" config --get core.sparseCheckout)" = true ]; then
  git -C "$CREATOR" sparse-checkout add release-gate/suites/execution/qwen-streamed release-gate/suites/execution/gpu-ready \
    || fail "fixtures not checked out"
fi
cmp -s "$X/qwen-streamed/ci_qwen_streamed/_prepared.py" "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "qwen fixture is not q's _prepared.py"
[ -s "$X/gpu-ready/ci_gpu_ready/__init__.py" ] || fail "no gpu-ready fixture"
grep -q '"webp"' "$PACKAGES/w/qwen-image-2/qwen_image_2/__init__.py" || fail "w is not #657's WebP output"
[ -s "$PACKAGES/m/sdxl/sdxl/__init__.py" ] && [ -s "$PACKAGES/s/sdxl/sdxl/_prepared.py" ] || fail "no sdxl trees"
DEV=$(command -v cozy); say "runner cozy: $("$DEV" --version) $(sha256sum "$DEV" | cut -c1-16)"
say "trees: creator $(git -C "$CREATOR" rev-parse --short HEAD), m $(git -C "$PACKAGES/m" rev-parse --short HEAD), q $(git -C "$PACKAGES/q" rev-parse --short HEAD), s $(git -C "$PACKAGES/s" rev-parse --short HEAD), w $(git -C "$PACKAGES/w" rev-parse --short HEAD)"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
echo '{"number":3,"status":"completed","result":{"passed":true},"measurements":{"execution":{"gpus":[{"pid":9}]},"memory":{"stats":{"varena":[{"stats":{"moved":{"store":{"bytes":7}}}}]}}}}' > "$OUT/probe.json"
[ "$(facts probe | jq -r '.moved')" = 7 ] && [ "$(pid_of probe)" = 9 ] && passed probe || fail "judges"
quote "$(param traffic_gb 60)" "$(param need_s 3600)"
rent

want="0.13.1 0.34.2 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.2 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
say "cohort: $NAME runs $want"

# 1: reduced Qwen cells.
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
fi

# 2: production TensorD: the ready request behind a fresh Qwen-Image-2 fetch (#657's WebP output).
step ready-warm run "$X/gpu-ready/gpu" tag=warm --on "$NAME" --gpus=1; show ready-warm
gated base w ./qwen-image-2/generate "prompt=a glass terrarium with moss and a tiny ceramic deer, soft window light" "seed:=$((S + 201))" megapixels:=1
passed base-w || RC=1
for f in "$OUT"/out/base-w/*; do [ -f "$f" ] && say "#657 output $(file -b "$f" | cut -c1-70) $(stat -c %s "$f") bytes ${f#$OUT/}"; done

# 3: Qwen-Image-2 at 1 MP: master (eager) vs #662, cold (same request) then warm (fresh requests).
QP="a lighthouse on a basalt cliff at blue hour, waves breaking below, long exposure photograph"
full qe-cold m ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 301))" megapixels:=1
full qn-cold q ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 301))" megapixels:=1
full qe-warm m ./qwen-image-2/generate "prompt=a bowl of ramen with a soft egg, steam rising, overhead food photograph" "seed:=$((S + 302))" megapixels:=1
full qn-warm q ./qwen-image-2/generate "prompt=a red vintage bicycle leaning on a yellow wall, afternoon sun" "seed:=$((S + 303))" megapixels:=1

# 4: #2154's TensorD (the runner's dev cozy): the ready request behind a fresh SDXL fetch.
step tensord-2154 machine update "$NAME" --tensord "$DEV" -y || fail "tensord-2154: machine update failed"
say "tensord-2154: $(cozy machine show "$NAME" --json | jq -c '{version: .status.version, dev: .status.dev, runtime: .status.runtime, tensorfs: .status.tensorfs}')"
step ready-warm2 run "$X/gpu-ready/gpu" tag=warm2 --on "$NAME" --gpus=1; show ready-warm2
gated gated m ./sdxl/generate "prompt=a lantern-lit alley in Kyoto after rain, cinematic photograph" "seed:=$((S + 401))" steps:=30

# 5: SDXL: master vs #661.
SP="a red fox resting beneath snow-covered pine branches at dawn, detailed natural photograph"
full se-cold m ./sdxl/generate "prompt=$SP" "seed:=$((S + 501))" steps:=30
full sn-cold s ./sdxl/generate "prompt=$SP" "seed:=$((S + 501))" steps:=30
full se-warm m ./sdxl/generate "prompt=a paper boat on a rain puddle, city reflections, macro photograph" "seed:=$((S + 502))" steps:=30
full sn-warm s ./sdxl/generate "prompt=a copper teapot on a wooden shelf, window light, still life" "seed:=$((S + 503))" steps:=30
for f in "$OUT"/out/*/*; do [ -f "$f" ] && say "output $(file -b "$f" | cut -c1-60) $(sha256sum "$f" | cut -c1-16) ${f#$OUT/}"; done
[ "$RC" = 0 ] || fail "see results.txt"
