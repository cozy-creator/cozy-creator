# Selected native-loop, kernel readiness and memory-pressure cells for the Runtime 0.35 / TensorFS 0.17 cohort.
# Sourced by gate.sh --cells after the selected model suite; --only still applies to these names.
# No H3 request, independent rental or SDK update is introduced here.
CORE_READY_TARGET=$HERE/suites/execution/kernel-ready-publication/cpu

cell kernel-ready-late-success "$CORE_READY_TARGET" mode=late-success seed:=$((R + 311))
cell kernel-ready-late-error "$CORE_READY_TARGET" mode=late-error seed:=$((R + 312))
cell kernel-ready-dropped-slot "$CORE_READY_TARGET" mode=dropped-slot seed:=$((R + 313))

# author.denoise_loop through real packages (xs-127), each judged by its run record's measurements.execution.loops[0]
# (the GPU memory limit cannot hold stock-PyTorch execution yet, so each memory case takes the card it needs):
#   loop-resident   Qwen-Image-2.1 int8 1 MP records resident (a card of 20 GiB or more);
#   loop-streamed   on a card of 12 GiB or less it streams, loading each block natively;
#   loop-retry      on a 20-26 GiB card, Qwen 4 MP after SDXL: the resident attempt runs out of memory and the loop
#                   continues streamed;
#   loop-cancel     a run canceled inside its native loop ends canceled, and the next call completes.
LQ=$(pkg qwen-image-2 generate)
GIB=$("$CLI" --json machine show "$M" 2>/dev/null | jq '[.status.gpus[].memory_bytes] | min / 1073741824 | floor')
# loop_expect <cell> <jq condition on its record>: an expect row with the loop's mode, native steps, blocks, refusals.
loop_expect() {
  local row
  row=$(run_get "$OUT" "$1" '.measurements.execution.loops[0] | "\(.mode) native \(.native_steps) blocks \(.blocks) \(.refusals // [] | join("; ") | .[0:200])"')
  expect "$1" "$(jq -e "$2" "$OUT/$1.json" > /dev/null 2>&1 && echo PASS || echo FAIL)" \
    "run $(cat "$1.number" 2> /dev/null) $(run_get "$OUT" "$1" .status): $row"
}
if [ -z "$ONLY" ] || [[ $ONLY == *",loop-"* || $ONLY == *",qwen-image-2,"* ]]; then
  if [ "$GIB" -ge 20 ]; then
    cell loop-resident "$LQ" prompt="$OBJECT, a resident loop" megapixels:=1.0 seed:=$((R + 321)) --lane int8 --gpus=1
    loop_expect loop-resident '.status == "completed" and .measurements.execution.loops[0].mode == "resident"'
  fi
  if [ "$GIB" -le 12 ]; then
    cell loop-streamed "$LQ" prompt="$OBJECT, a streamed loop" megapixels:=1.0 seed:=$((R + 322)) --lane int8 --gpus=1
    loop_expect loop-streamed '.status == "completed" and (.measurements.execution.loops[0] | .mode == "streamed" and .blocks > 0)'
  fi
  if [ "$GIB" -ge 20 ] && [ "$GIB" -le 26 ]; then
    cell loop-retry-sdxl "$(pkg sdxl generate)" prompt="$OBJECT before a loop that retries" seed:=$((R + 326)) --lane int8 --gpus=1
    cell loop-retry "$LQ" prompt="$OBJECT, a loop that retries" megapixels:=4.0 seed:=$((R + 323)) --lane int8 --gpus=1
    loop_expect loop-retry '.status == "completed" and (.measurements.execution.loops[0] | .mode == "streamed"
      and (.refusals[0] // "" | startswith("resident")))'
  fi
  # Cancel once the loop is past its eager first steps: the native program is running.
  run_submit "$OUT" loop-cancel run "$LQ" prompt="$OBJECT, a canceled loop" megapixels:=1.0 seed:=$((R + 324)) \
    --lane int8 --gpus=1 --on "$M" --out "$OUT/out/loop-cancel" > /dev/null
  n=$(cat loop-cancel.number)
  while run_show "$OUT" loop-cancel "$n"; [ "$(run_get "$OUT" loop-cancel .status)" != completed ] \
    && [ "$(run_get "$OUT" loop-cancel '.progress.completed // 0')" -lt 3 ] && kill -0 "$(cat loop-cancel.pid)" 2> /dev/null; do
    sleep 0.5
  done
  "$CLI" --json run cancel "$n" > loop-cancel-cancel.json 2>&1
  run_wait "$OUT" loop-cancel || true
  loop_expect loop-cancel '.status == "canceled"'
  cell loop-after-cancel "$LQ" prompt="$OBJECT, after a canceled loop" megapixels:=1.0 seed:=$((R + 325)) --lane int8 --gpus=1
  loop_expect loop-after-cancel '.status == "completed"'
fi
