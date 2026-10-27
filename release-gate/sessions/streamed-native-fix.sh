# streamed-native-fix: one 8 GB rental (RTX 3070) proving that a native stage capture borrows its component scope's
# common block (cozy-runtime opus/stage-capture-lends-common-20261027), so #662's streamed Qwen engages in a real call.
# Qwen-Image-2 1 MP / 40 steps / int8 from packages master, on released Runtime 0.34.5 (eager: the stage refuses), then
# on the fix wheel, same pod. e2 and s1 share prompt and seed (the image match). packages_refs: q=<packages master>.
# params: runtime "<URL#sha256=…>" (the fix wheel), traffic_gb (25), need_s (2400)
. "$(dirname "$0")/lib.sh"
fetch() {
  local f=$OUT/wheels/$(basename "${1%%#*}" | sed 's/%2B/+/g')
  [[ $1 == https://*#sha256=* ]] || { echo "$1 is not https://…#sha256=" >&2; return 1; }
  mkdir -p "$OUT/wheels"
  [ -f "$f" ] || curl -fsSL --retry 5 --retry-all-errors --speed-limit 1024 --speed-time 60 "${1%%#*}" -o "$f" || { echo "cannot fetch ${1%%#*}" >&2; return 1; }
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${1##*#sha256=}" ] || { echo "$f does not hash to ${1##*#sha256=}" >&2; return 1; }
  echo "$f"
}
reports() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '[.status.version, .status.runtime, .status.tensorfs] | join(" ")'; }
facts() {
  jq -c '{status, run: .number, exec_ms: .execution_ms,
    stages: ((.measurements.attribution.stages // {}) | with_entries(select(.value | type == "object" and has("total_ms"))) | map_values(.total_ms | floor)),
    moved: ([.measurements.memory.stats.varena[]?.stats.moved // {} | to_entries[] | .value.bytes] | add),
    metrics: ([.measurements.observations[]? | select(.name | startswith("request_path")) | {(.name): .value}] | add),
    driver_peak: ([.measurements.memory.driver[]?.peak_bytes] | max), software: .measurements.software,
    reason: (.reason // null | if . then {code, message: (.message // "" | .[0:300])} else null end)}' "$OUT/$1.json" 2>/dev/null
}
cell() {
  local n=$1 prompt=$2 seed=$3 N
  step "$n" run ./qwen-image-2/generate "prompt=$prompt" "seed:=$seed" megapixels:=1 --on "$NAME" --json --out "$OUT/out/$n"
  N=$(grep -m1 '"run.created"' "$OUT/$n.txt" | jq -r '.run.number')
  [ -n "$N" ] && cozy run show "$N" --once --json > "$OUT/$n.json" 2>/dev/null
  say "$n: $(facts "$n")"
  say "$n log: $(grep -h '"run.log"' "$OUT/$n.txt" | jq -r '.text // empty' 2>/dev/null | grep -iE 'eager|native|streamed|capab|capacity|refus' | head -6 | tr '\n' '|' | cut -c1-1200)"
  jq -e '.status == "completed"' "$OUT/$n.json" >/dev/null 2>&1 || RC=1
}
streamed() { jq -r '[.measurements.observations[]? | select(.name == "request_path.denoise_streamed") | .value][0] // "none"' "$OUT/$1.json" 2>/dev/null; }
collect() {
  [ -n "$NAME" ] || return 0
  timeout 120 cozy machine logs "$NAME" tensord -n 0 > "$OUT/machine-tensord.log" 2>&1
  timeout 120 cozy machine logs "$NAME" memory -n 0 > "$OUT/machine-memory.log" 2>&1
}
trap 's=$?; collect; (exit $s); finish' EXIT

grep -q '^class _StreamedDenoise' "$PACKAGES/q/qwen-image-2/qwen_image_2/_prepared.py" || fail "q lacks #662"
WHL=$(fetch "$(param runtime)") || fail "runtime wheel"
say "tree: q $(git -C "$PACKAGES/q" rev-parse --short HEAD); fix wheel $(basename "$WHL") $(sha256sum "$WHL" | cut -c1-12)"
S=$(( $(date +%s) % 100000000 * 10 ))
quote "$(param traffic_gb 25)" "$(param need_s 2400)"
rent
want="0.13.1 0.34.5 0.17.2"
if [ "$(reports)" != "$want" ]; then
  step cohort machine update "$NAME" --tensord 0.13.1 --runtime 0.34.5 --tensorfs 0.17.2 -y || fail "cohort: machine update failed"
  for _ in $(seq 12); do [ "$(reports)" = "$want" ] && break; sleep 5; done
fi
[ "$(reports)" = "$want" ] || fail "$NAME reports $(reports), not $want"
cozy machine show "$NAME" --json > "$OUT/machine-released.json" 2>&1
say "released: $NAME runs $want"
cd "$PACKAGES/q" || fail "no q"
cell e1 "a quiet harbor at dawn with fishing boats, photograph" $((S + 1))
cell e2 "a red vintage bicycle leaning on a yellow wall, afternoon sun" $((S + 2))
step fix --json machine update "$NAME" --runtime "$WHL" -y || fail "the fix wheel did not go on"
cozy machine show "$NAME" --json > "$OUT/machine-fix.json" 2>&1
say "fix: $NAME runs $(reports); $(jq -c '.status | with_entries(select(.key | test("runtime|sdk|sha|digest")))' "$OUT/machine-fix.json" 2>/dev/null | cut -c1-400)"
cell s1 "a red vintage bicycle leaning on a yellow wall, afternoon sun" $((S + 2))
cell s2 "a bowl of ramen with a soft egg, steam rising, overhead food photograph" $((S + 3))
cd "$OUT" || fail "no $OUT"
say "denoise_streamed: e2 $(streamed e2) s1 $(streamed s1) s2 $(streamed s2)"
[ "$(streamed s1)" = 1 ] && [ "$(streamed s2)" = 1 ] || RC=1
