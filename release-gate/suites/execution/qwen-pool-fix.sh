# Same-pod proof (release lane): Qwen-Image-2 4MP after SDXL, as users run it, twice, on the packages_ref tree (the
# native denoise program released before the VAE decode). 0.1.2 failed this sequence (40503774826: VAE decode OOM,
# 2.38 GiB held by captured graphs). Sourced by gate.sh --cells; `only` names the four cells.
S=sdxl/generate
Q=$(pkg qwen-image-2 generate)
QP="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
cell pf-sdxl-a "$S" prompt="$SCENE, photo, morning" seed:=$((R + 391))
cell pf-qwen-1 "$Q" prompt="$QP Morning light." seed:=$((R + 392))
cell pf-sdxl-b "$S" prompt="$SCENE, photo, evening" seed:=$((R + 393))
cell pf-qwen-2 "$Q" prompt="$QP Evening light." seed:=$((R + 394))
for n in pf-qwen-1 pf-qwen-2; do
  [ -f "$n.json" ] || continue
  expect "$n-memory" "$([ "$(run_get "$OUT" "$n" .status)" = completed ] && echo PASS || echo FAIL)" \
    "$(jq -r '[.status, (.execution_ms|tostring), ([.. | objects | select(has("peak_bytes")) | .peak_bytes] | max / 1048576 | floor | tostring) + " MiB driver peak", (.reason.message // "" | .[0:200])] | join("; ")' "$n.json")"
done
