# Pre-roll sanity (release lane): SDXL bf16 twice on one executor. The second request must run the resident native
# pipeline (packages #668), the path driver 615 crashed in (cozy-runtime #1860). Sourced by gate.sh --cells;
# `only` names sdxl-bf16-1, sdxl-bf16-2.
S=$(pkg sdxl generate)
cell sdxl-bf16-1 "$S" prompt="$SCENE, photo, first light" seed:=$((R + 371)) --lane bf16
cell sdxl-bf16-2 "$S" prompt="$OBJECT on a windowsill, photo, rain outside" seed:=$((R + 372)) --lane bf16
if [ -f sdxl-bf16-2.json ]; then
  nat=$(jq -r '[.. | objects | select(has("stages")) | .stages | select(has("native_pipeline")) | .native_pipeline.count] | add // 0' sdxl-bf16-2.json 2>/dev/null)
  pid1=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-1.json 2>/dev/null)
  pid2=$(jq -r '.measurements.execution.gpus[0].pid // empty' sdxl-bf16-2.json 2>/dev/null)
  st=$(run_get "$OUT" sdxl-bf16-2 .status)
  expect sdxl-native-second "$([ "$st" = completed ] && [ "${nat:-0}" -gt 0 ] && [ -n "$pid1" ] && [ "$pid1" = "$pid2" ] && echo PASS || echo FAIL)" \
    "request 2 $st, native observations ${nat:-0}, executor $pid1 -> $pid2; $(grep -m1 -oE '[A-Za-z_ ]*(SIGSEGV|signal: 11)[^"]{0,120}' sdxl-bf16-2.json 2>/dev/null)"
fi
