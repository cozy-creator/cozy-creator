# Same-pod A/B (release lane): Qwen-Image-2 4MP after SDXL, as users run it, on qwen-image-2 0.1.1 and then 0.1.2 with
# a machine restart between the arms (every executor new). Sourced by gate.sh --cells; `only` names the four cells.
S=$(pkg sdxl generate)
QP="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
cell ab-sdxl-a "$S" prompt="$SCENE, photo, morning" seed:=$((R + 381))
cell ab-qwen-011 "qwen-image-2/generate@0.1.1" prompt="$QP Morning light." seed:=$((R + 382))
if [ -z "$ONLY" ] || [[ $ONLY == *",ab-sdxl-b,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > ab-restart.log 2>&1; log "restarted $M between arms: $(grep -E '^status' ab-restart.log)"
fi
cell ab-sdxl-b "$S" prompt="$SCENE, photo, evening" seed:=$((R + 383))
cell ab-qwen-012 "qwen-image-2/generate@0.1.2" prompt="$QP Evening light." seed:=$((R + 384))
for n in ab-qwen-011 ab-qwen-012; do
  [ -f "$n.json" ] || continue
  expect "$n-memory" "$([ "$(run_get "$OUT" "$n" .status)" = completed ] && echo PASS || echo FAIL)" \
    "$(jq -r '[.status, (.execution_ms|tostring), ([.. | objects | select(has("peak_bytes")) | .peak_bytes] | max / 1048576 | floor | tostring) + " MiB driver peak", (.reason.message // "" | .[0:200])] | join("; ")' "$n.json")"
done
