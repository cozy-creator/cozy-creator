# Loops' captured step graphs and kernel readiness, for the `execution` suite.
# --only still applies to these names.
# No H3 request, independent rental or SDK update is introduced here.
# Kernel readiness publication, the Runtime's own exerciser (`bench.kernel_ready`, CPU).
CORE_READY_TARGET=$HERE/suites/execution/drive/kernel_ready

cell kernel-ready-late-success "$CORE_READY_TARGET" mode=late-success seed:=$((R + 311))
cell kernel-ready-late-error "$CORE_READY_TARGET" mode=late-error seed:=$((R + 312))
cell kernel-ready-dropped-slot "$CORE_READY_TARGET" mode=dropped-slot seed:=$((R + 313))

# A package's plain loop through real packages, each judged by its run record's measurements.residency.graphs (the
# Runtime captures a model's step graph; denoise_loop and execution.loops went in cozy-runtime 4fab07a57, Runtime 0.43.1):
#   loop-kept       SDXL on a card of 20 GiB or more: the first call captures the unet's step graph, the later calls
#                   capture nothing and replay every step;
#   loop-cancel     a run canceled inside its denoise loop ends canceled, and the next call completes.
LQ=$(pkg qwen-image-2 generate)
GIB=$("$CLI" --json machine show "$M" 2>/dev/null | jq '[.status.gpus[].memory_bytes] | min / 1073741824 | floor')
# graph_expect <cell> <model> <jq condition on its record>: an expect row with the model's captures, replays and eager steps.
graph_expect() {
  local row
  row=$(run_get "$OUT" "$1" ".measurements.residency.graphs[\"$2\"] | \"$2 captures \(.captures) replays \(.replays) eager \(.eager.calls)\"")
  expect "$1" "$(jq -e "$3" "$OUT/$1.json" > /dev/null 2>&1 && echo PASS || echo FAIL)" \
    "run $(cat "$1.number" 2> /dev/null) $(run_get "$OUT" "$1" .status): ${row:-no $2 graphs}"
}
if [ -z "$ONLY" ] || [[ $ONLY == *",loop-"* || $ONLY == *",qwen-image-2,"* || $ONLY == *",sdxl,"* ]]; then
  if [ "$GIB" -ge 20 ]; then
    for i in 1 2 3; do
      cell "loop-kept-$i" "$(pkg sdxl generate)" prompt="$OBJECT, a kept loop, call $i" seed:=$((R + 320 + i)) --lane int8 --gpus=1
    done
    graph_expect loop-kept-3 unet '.status == "completed" and (.measurements.residency.graphs.unet
      | .captures == 0 and .capture_attempts == 0 and .eager.calls == 0 and .replays > 0)'
  fi
  # Cancel once the denoise loop is past its first steps: the prompt upsampler's decode counts its tokens before it.
  run_submit "$OUT" loop-cancel run "$LQ" prompt="$OBJECT, a canceled loop" megapixels:=1 seed:=$((R + 324)) \
    --lane int8 --gpus=1 --on "$M" > /dev/null
  n=$(cat loop-cancel.number)
  while run_show "$OUT" loop-cancel "$n"; [ "$(run_get "$OUT" loop-cancel .status)" != completed ] \
    && ! jq -e '(.progress.stage // "" | startswith("generating image")) and (.progress.completed // 0) >= 3' loop-cancel.json > /dev/null 2>&1 \
    && kill -0 "$(cat loop-cancel.pid)" 2> /dev/null; do
    sleep 0.5
  done
  # By its name: a bare number is its project's, and this folder is not the project (cl-1028).
  "$CLI" --json run cancel "$(run_ref "$OUT" loop-cancel "$n")" > loop-cancel-cancel.json 2>&1
  run_wait "$OUT" loop-cancel || true
  graph_expect loop-cancel model.transformer '.status == "canceled"'
  cell loop-after-cancel "$LQ" prompt="$OBJECT, after a canceled loop" megapixels:=1 seed:=$((R + 325)) --lane int8 --gpus=1
  graph_expect loop-after-cancel model.transformer '.status == "completed"'
fi
