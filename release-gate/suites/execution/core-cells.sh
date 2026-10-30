# Selected native-loop, kernel readiness and memory-pressure cells for the Runtime 0.35 / TensorFS 0.17 cohort.
# Sourced by gate.sh --cells after the selected model suite; --only still applies to these names.
# No H3 request, independent rental or SDK update is introduced here.
CORE_READY_TARGET=$HERE/suites/execution/kernel-ready-publication/cpu

cell kernel-ready-late-success "$CORE_READY_TARGET" mode=late-success seed:=$((R + 311))
cell kernel-ready-late-error "$CORE_READY_TARGET" mode=late-error seed:=$((R + 312))
cell kernel-ready-dropped-slot "$CORE_READY_TARGET" mode=dropped-slot seed:=$((R + 313))

# author.denoise_loop through real packages (xs-127), each judged by its run record's measurements.execution.loops[0]
# (the GPU memory limit cannot hold stock-PyTorch execution yet, so each memory case takes the card it needs). Only a
# program some call keeps is recorded (cozy-runtime #1891 and the streamed rule after it):
#   loop-kept       SDXL's keyed loop on a card of 20 GiB or more: the first call runs eagerly, the idle period after it
#                   records the whole call, the later calls run it natively from start to end (reused);
#   loop-eager      Qwen-Image-2.1 int8 1 MP, a loop no call keeps, runs eagerly: resident on a large card, streamed
#                   on a small one;
#   loop-cancel     a run canceled inside its loop ends canceled, and the next call completes.
LQ=$(pkg qwen-image-2 generate)
GIB=$("$CLI" --json machine show "$M" 2>/dev/null | jq '[.status.gpus[].memory_bytes] | min / 1073741824 | floor')
# loop_expect <cell> <jq condition on its record>: an expect row with the loop's mode, native steps, reuse, refusals.
loop_expect() {
  local row
  row=$(run_get "$OUT" "$1" '.measurements.execution.loops[0] | "\(.mode) native \(.native_steps) reused \(.reused) \(.eager // "") \(.refusals // [] | join("; ") | .[0:200])"')
  expect "$1" "$(jq -e "$2" "$OUT/$1.json" > /dev/null 2>&1 && echo PASS || echo FAIL)" \
    "run $(cat "$1.number" 2> /dev/null) $(run_get "$OUT" "$1" .status): $row"
}
if [ -z "$ONLY" ] || [[ $ONLY == *",loop-"* || $ONLY == *",qwen-image-2,"* || $ONLY == *",sdxl,"* ]]; then
  if [ "$GIB" -ge 20 ]; then
    for i in 1 2 3; do
      cell "loop-kept-$i" "$(pkg sdxl generate)" prompt="$OBJECT, a kept loop, call $i" seed:=$((R + 320 + i)) --lane int8 --gpus=1
    done
    loop_expect loop-kept-3 '.status == "completed" and (.measurements.execution.loops[0] | .mode == "resident" and .reused)'
  fi
  cell loop-eager "$LQ" prompt="$OBJECT, an eager loop" megapixels:=1.0 seed:=$((R + 326)) --lane int8 --gpus=1
  loop_expect loop-eager '.status == "completed" and (.measurements.execution.loops[0] | .mode == "eager"
    and (.eager // "" | endswith("and no call keeps the program")))'
  # Cancel once the loop is past its first steps.
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
