# streamed-native-8gb: one 8 GB rental (RTX 3070), where even Qwen-Image-2's int8 transformer (~7 GB) streams:
# Qwen-Image-2 1 MP / 40 steps / int8, packages master (eager) vs #662, cold (same request) then warm (fresh requests),
# on the production cohort. packages_refs: m=<master> q=<#662>. params: traffic_gb (25), need_s (3600)
. "$(dirname "$0")/lib.sh"
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
collect() {
  [ -n "$NAME" ] || return 0
  timeout 120 cozy machine logs "$NAME" tensord -n 0 > "$OUT/machine-tensord.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/machine-memory.log" 2>&1
}
trap 's=$?; collect; (exit $s); finish' EXIT

grep -q '^class _StreamedDenoise' "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "q is not #662"
[ -s "$PACKAGES/m/qwen-image-2/qwen_image_2/model.py" ] || fail "no master tree"
say "trees: m $(git -C "$PACKAGES/m" rev-parse --short HEAD), q $(git -C "$PACKAGES/q" rev-parse --short HEAD)"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/f\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
echo '{"number":3,"status":"completed","result":{"passed":true},"measurements":{"execution":{"gpus":[{"pid":9}]},"memory":{"stats":{"varena":[{"stats":{"moved":{"store":{"bytes":7}}}}]}}}}' > "$OUT/probe.json"
[ "$(facts probe | jq -r '.moved')" = 7 ] && passed probe || fail "judges"
quote "$(param traffic_gb 25)" "$(param need_s 3600)"
rent

want="0.13.1 0.34.2 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.2 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
say "cohort: $NAME runs $want"
QP="a stone footbridge over a mountain stream in autumn, morning mist, landscape photograph"
full qe-cold m ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 601))" megapixels:=1
full qn-cold q ./qwen-image-2/generate "prompt=$QP" "seed:=$((S + 601))" megapixels:=1
full qe-warm m ./qwen-image-2/generate "prompt=a jar of honey with a wooden dipper on linen, soft daylight" "seed:=$((S + 602))" megapixels:=1
full qn-warm q ./qwen-image-2/generate "prompt=a vintage typewriter on a cluttered writer's desk, lamp light" "seed:=$((S + 603))" megapixels:=1
for f in "$OUT"/out/*/*; do [ -f "$f" ] && say "output $(file -b "$f" | cut -c1-60) $(sha256sum "$f" | cut -c1-16) ${f#$OUT/}"; done
[ "$RC" = 0 ] || fail "see results.txt"
