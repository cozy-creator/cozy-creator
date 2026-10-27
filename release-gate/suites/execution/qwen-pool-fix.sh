# Same-pod proof (release lane): Qwen-Image-2 4MP on 0.1.1 (cold, then warm), a machine restart, then SDXL and Qwen
# 4MP on the packages_ref tree (0.1.2 + the native denoise program released before the VAE decode; cold, then warm).
# 0.1.2 failed its Qwen after SDXL (40503774826: VAE decode OOM, 2.38 GiB held by captured graphs).
# Sourced by gate.sh --cells; `only` names the five cells.
S=sdxl/generate
Q=$(pkg qwen-image-2 generate)
QP="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
cell pf-q011-1 "qwen-image-2/generate@0.1.1" prompt="$QP Morning light." seed:=$((R + 391))
cell pf-q011-2 "qwen-image-2/generate@0.1.1" prompt="$QP Noon light." seed:=$((R + 392))
if [ -z "$ONLY" ] || [[ $ONLY == *",pf-sdxl,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > pf-restart.log 2>&1; log "restarted $M between arms: $(grep -E '^status' pf-restart.log)"
fi
cell pf-sdxl "$S" prompt="$SCENE, photo, evening" seed:=$((R + 393))
cell pf-qfix-1 "$Q" prompt="$QP Evening light." seed:=$((R + 394))
cell pf-qfix-2 "$Q" prompt="$QP Night light." seed:=$((R + 395))
for n in pf-q011-1 pf-q011-2 pf-qfix-1 pf-qfix-2; do
  [ -f "$n.json" ] || continue
  expect "$n-memory" "$([ "$(run_get "$OUT" "$n" .status)" = completed ] && echo PASS || echo FAIL)" \
    "$(jq -r '[.status, "exec " + (.execution_ms|tostring) + " ms", "generating " + ((.measurements.attribution.stages["generating image"].total_ms // 0) | floor | tostring) + " ms", "decode " + ((.measurements.attribution.stages["decoding image"].total_ms // 0) | floor | tostring) + " ms", ([.. | objects | select(has("peak_bytes")) | .peak_bytes] | max / 1048576 | floor | tostring) + " MiB driver peak", ([.measurements.observations[]? | select(.name | startswith("request_path.denoise")) | "\(.name | ltrimstr("request_path."))=\(.value)"] | join(" ")), (.reason.message // "" | .[0:200])] | join("; ")' "$n.json")"
done
