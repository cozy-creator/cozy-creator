# streamed-native-8gb: one 8 GB rental (RTX 3070), where even Qwen-Image-2's int8 transformer (~7 GB) streams:
#   1. production cohort: a weightless ready GPU request submitted while Qwen-Image-2 int8 fetches fresh (the baseline
#      for creator #2154), that run being master's cold Qwen; then Qwen 1 MP / 40 steps / int8, master vs #662;
#   2. #2154's TensorD (the runner's dev cozy): the same ready request while the bf16 lane fetches fresh.
# packages_refs: m=<master> q=<#662>. params: traffic_gb (55), need_s (3600)
. "$(dirname "$0")/lib.sh"
CREATOR=$(cd "$(dirname "$0")/../.." && pwd) X=$CREATOR/release-gate/suites/execution
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
full() {
  local n=$1 tree=$2; shift 2
  cd "$PACKAGES/$tree" || fail "no packages/$tree"
  step "$n" run "$@" --on "$NAME" --out "$OUT/out/$n"; show "$n"
  cd "$OUT" || fail "no $OUT"
  say "$n ($tree $(git -C "$PACKAGES/$tree" rev-parse --short HEAD)): $(facts "$n")"
  passed "$n" || RC=1
}
# gated <label> <tree> <target> <args…>: the target from a packages tree in the background (its weights fetching
# fresh), then, once that run is in progress, the warm weightless ready request; both records kept.
gated() {
  local l=$1 tree=$2 p N='' st=''; shift 2
  cd "$PACKAGES/$tree" || fail "no packages/$tree"
  cozy run "$@" --on "$NAME" --out "$OUT/out/$l-w" > "$OUT/$l-w.txt" 2>&1 & p=$!
  cd "$OUT" || fail "no $OUT"
  for _ in $(seq 300); do
    N=$(awk '$1=="run" && $3=="·" && $4!="sync" && $4!="install" {print $2; exit}' "$OUT/$l-w.txt")
    [ -n "$N" ] && st=$(cozy run show "$N" --once --json 2>/dev/null | jq -r '.status // empty')
    [ "$st" = in_progress ] || [ "$st" = queued ] && break; sleep 1
  done
  [ "$st" = in_progress ] || [ "$st" = queued ] || { kill "$p"; fail "$l: the fetching run was never accepted (run ${N:-none} ${st:-none})"; }
  sleep 5  # its required inputs are fetching
  say "$l: run $N accepted: $(cozy run show "$N" --once --json 2>/dev/null | jq -c '{status, machine_state, accepted_at_ms, started_at_ms}')"
  step "$l-r" run "$X/gpu-ready/gpu" "tag=$l" --on "$NAME" --gpus=1; show "$l-r"
  say "$l: ready done; fetching run now $(cozy run show "$N" --once --json 2>/dev/null | jq -c '{status, machine_state}')"
  wait "$p"; show "$l-w"
  say "$l fetching run: $(facts "$l-w")"
  say "$l ready run: $(facts "$l-r") entered_ms $(jq -r '.result.entered_ms // empty' "$OUT/$l-r.json")"
  local first settled; settled=$(jq -r .settled_at_ms "$OUT/$l-r.json")
  first=$(jq -r '[.measurements.attribution.stages // .measurements.attribution // {} | to_entries[] | .value.started_unix_ms? // empty] | min // empty' "$OUT/$l-w.json")
  say "$l verdict: ready settled $settled, fetching run's first stage $first: $( [ -n "$first" ] && [ -n "$settled" ] && [ "$settled" -lt "$first" ] && echo 'READY RAN WHILE THE INPUTS FETCHED' || echo 'READY WAITED BEHIND THE FETCHING RUN')"
}
collect() {
  [ -n "$NAME" ] || return 0
  timeout 120 cozy machine logs "$NAME" tensord -n 0 > "$OUT/machine-tensord.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/machine-memory.log" 2>&1
}
trap 's=$?; collect; (exit $s); finish' EXIT

if [ "$(git -C "$CREATOR" config --get core.sparseCheckout)" = true ]; then
  git -C "$CREATOR" sparse-checkout add release-gate/suites/execution/gpu-ready || fail "fixture not checked out"
fi
[ -s "$X/gpu-ready/ci_gpu_ready/__init__.py" ] || fail "no gpu-ready fixture"
DEV=$(command -v cozy); say "runner cozy: $("$DEV" --version)"
grep -q '^class _StreamedDenoise' "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "q is not #662"
[ -s "$PACKAGES/m/qwen-image-2/qwen_image_2/model.py" ] || fail "no master tree"
say "trees: m $(git -C "$PACKAGES/m" rev-parse --short HEAD), q $(git -C "$PACKAGES/q" rev-parse --short HEAD)"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
echo '{"number":3,"status":"completed","result":{"passed":true},"measurements":{"execution":{"gpus":[{"pid":9}]},"memory":{"stats":{"varena":[{"stats":{"moved":{"store":{"bytes":7}}}}]}}}}' > "$OUT/probe.json"
[ "$(facts probe | jq -r '.moved')" = 7 ] && passed probe || fail "judges"
quote "$(param traffic_gb 55)" "$(param need_s 3600)"
rent

want="0.13.1 0.34.2 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.2 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
say "cohort: $NAME runs $want"
QP="a stone footbridge over a mountain stream in autumn, morning mist, landscape photograph"
step ready-warm run "$X/gpu-ready/gpu" tag=warm --on "$NAME" --gpus=1; show ready-warm
gated base m ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 601))" megapixels:=1
passed base-w || RC=1
full qn-cold q ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 601))" megapixels:=1
full qe-warm m ./qwen-image-2/generate "prompt=a jar of honey with a wooden dipper on linen, soft daylight" "seed:=$((S + 602))" megapixels:=1
full qn-warm q ./qwen-image-2/generate "prompt=a vintage typewriter on a cluttered writer's desk, lamp light" "seed:=$((S + 603))" megapixels:=1

# 2: #2154's TensorD: the ready request while the bf16 lane fetches fresh.
step tensord-2154 machine update "$NAME" --tensord "$DEV" -y || fail "tensord-2154: machine update failed"
say "tensord-2154: $(cozy machine show "$NAME" --json | jq -c '{version: .status.version, dev: .status.dev}')"
step ready-warm2 run "$X/gpu-ready/gpu" tag=warm2 --on "$NAME" --gpus=1; show ready-warm2
gated gated m ./qwen-image-2/generate "prompt=a fishing village at dusk with lanterns on the pier, painterly" "seed:=$((S + 604))" megapixels:=1 --lane bf16
passed gated-w || RC=1
for f in "$OUT"/out/*/*; do [ -f "$f" ] && say "output $(file -b "$f" | cut -c1-60) $(sha256sum "$f" | cut -c1-16) ${f#$OUT/}"; done
[ "$RC" = 0 ] || fail "see results.txt"
