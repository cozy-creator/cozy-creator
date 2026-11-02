# Suite `families`: the packages that send attention masks, on the families whose kernels serve none. A call a kernel's
# row refuses runs on SDPA and the record counts it. Escaped in Runtime 0.37.2 (runs 26242-26244, 2x H100): every
# qwen-image-2 request failed `flash-attn3 does not serve an attention mask` at its first step, because `runtime`
# rents only mma.sync, whose 8-bit kitchen-int8 serves masks. qwen-image-2 masks its prefill's text at step 0 only;
# ideogram-4 masks every call (at head dimension 256 it attends on SDPA today, so a new kernel there must hold).
case $SKU in
  h100*) masked=sageattention ;;
  b200*) masked=flash-attn4-fp8 ;;
  *) log "families has no masks=False kernel for $SKU"; return 1 ;;
esac
cell qwen-masked "$(pkg qwen-image-2 generate)" prompt="$OBJECT on a slate shelf, soft window light" seed:=$((R + 900))
cell qwen-masked-bf16 "$(pkg qwen-image-2 generate)" --lane bf16 prompt="$SCENE, seen through a rain-streaked window" seed:=$((R + 901))
# The pin waits for its build, so the masked step meets this kernel whatever the first calls' compile did.
cell qwen-masked-pin "$(pkg qwen-image-2 generate)" --lane int8 --attention-kernel "$masked" prompt="$OBJECT beside a lit candle, close photo" seed:=$((R + 902))
if [ -f qwen-masked-pin.json ]; then
  d=$(run_get "$OUT" qwen-masked-pin '[.measurements.execution.gpus[]?.attention.kernels[]? | select(.kernel == "sdpa") | .detail] | join(";")')
  k=$(run_get "$OUT" qwen-masked-pin '.measurements.execution.gpus[0].attention.observed')
  expect qwen-masked-fallback "$([ "$k" = "$masked" ] && grep -q "$masked does not serve an attention mask" <<<"$d" && echo PASS || echo FAIL)" \
    "qwen-image-2 pinned to $masked: site served ${k:-nothing}; SDPA's record: ${d:-none}"
fi
cell ideogram-masked "$(pkg ideogram-4 generate)" prompt="a hand-lettered poster that reads OPEN LATE, $SCENE" seed:=$((R + 903))
