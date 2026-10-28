# Runtime 0.35.0 + TensorFS 0.17.5 + cozy 0.13.7 pre-roll sanity, one GPU (release lane, proof-only). Sourced by gate.sh
# --cells after the runtime suite's selected cells: the denoise_loop cells (core-cells.sh), SDXL bf16 twice on one
# executor, Krea 2 Raw, Turbo and Turbo with a caller LoRA, and the H3 receipt.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/core-cells.sh"
S=$(pkg sdxl generate)
cell sdxl-bf16-1 "$S" prompt="$SCENE, photo, first light" seed:=$((R + 371)) --lane bf16
cell sdxl-bf16-2 "$S" prompt="$OBJECT on a windowsill, photo, rain outside" seed:=$((R + 372)) --lane bf16
if [ -f sdxl-bf16-2.json ]; then
  pid1=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-1.json 2>/dev/null)
  pid2=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-2.json 2>/dev/null)
  st=$(run_get "$OUT" sdxl-bf16-2 .status)
  expect sdxl-bf16-second "$([ "$st" = completed ] && [ -n "$pid1" ] && [ "$pid1" = "$pid2" ] && echo PASS || echo FAIL)" \
    "request 2 $st, executor $pid1 -> $pid2; loop $(run_get "$OUT" sdxl-bf16-2 '.measurements.execution.loops[0] | "\(.mode) native \(.native_steps) reused \(.reused)"')"
fi
K=$(pkg krea-2 generate) KT=$(pkg krea-2 generate_turbo)
cell krea "$K" prompt="$SCENE, photo" seed:=$((R + 381))
cell krea-turbo "$KT" prompt="$CHAR, portrait photo" seed:=$((R + 382))
cell krea-turbo-lora "$KT" prompt="$OBJECT, ink wash painting" seed:=$((R + 383)) \
  --lora "model:transformer=fidika/krea-2-darkbrush@sha256:95afffbed63209deecd40744849fa42e8ed61aaada1e52b09c033d447d428ed0,1.0"
if [ -f h3.json ]; then
  st=$(run_get "$OUT" h3 .status)
  expect h3-route "$([ "$st" = completed ] && echo PASS || echo FAIL)" \
    "$st; LoRA $(lora h3); route $(route h3); served $(served h3); exact $(exact_sites h3); vae $(vae h3); decode $(decode_s h3) s"
fi
