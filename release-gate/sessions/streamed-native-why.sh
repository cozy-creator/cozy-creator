# streamed-native-why: one 8 GB rental (RTX 3070) to read why #662's streamed Qwen denoise refuses there:
# Qwen-Image-2 1 MP / 40 steps / int8 from packages master (with #662), twice, keeping each run's event stream (its
# "eager denoise" log names the refusal). packages_refs: q=<packages master>. params: traffic_gb (20), need_s (1800)
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

grep -q '^class _StreamedDenoise' "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "q lacks #662"
say "tree: q $(git -C "$PACKAGES/q" rev-parse --short HEAD)"
quote "$(param traffic_gb 20)" "$(param need_s 1800)"
rent
want="0.13.1 0.34.2 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.2 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
say "cohort: $NAME runs $want"
cd "$PACKAGES/q" || fail "no q"
for n in 1 2; do
  step "why-$n" run ./qwen-image-2/generate "prompt=a quiet harbor at dawn with fishing boats, photograph $n" "seed:=$((S + n))" megapixels:=1 --on "$NAME" --json
  say "why-$n: $(grep -h '"run.log"' "$OUT/why-$n.txt" | jq -r '.text // empty' 2>/dev/null | grep -iE 'eager|native|streamed|capab|capacity|resident|refus|denoise' | head -12 | tr '\n' '|' | cut -c1-1500)"
done
cd "$OUT" || fail "no $OUT"
