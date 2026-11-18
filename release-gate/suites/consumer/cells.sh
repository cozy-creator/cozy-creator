# Suite `consumer`: the one fp8 lane on a card without fp8 tensor cores.
# - sdxl-fp8-float (a card over 8 GB below sm89): SDXL 1024 in its fp8 lane, each fp8.rowwise weight decoded to 16-bit for
#   torch's GEMM. Escaped in RC 0.27.0 (cr-338): cr-327 put int8.convrot's conv branch in the fp8 and mxfp8 decoders, which
#   read a `rotation_size` neither carries, so every such call failed KeyError 'rotation_size' (run FD8-2, RTX 3080 Ti). It
#   must complete.
GIB=$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+' || true)
SM=$(grep -oE '· sm[0-9]+ ·' machine.txt | head -1 | grep -oE '[0-9]+' || true)
if [ "${GIB:-0}" -gt 8 ] && [ "${SM:-89}" -lt 89 ] && { [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-fp8-float,"* || $ONLY == *",sdxl,"* ]]; }; then
  cell sdxl-fp8-float "$(pkg sdxl generate)" --lane fp8 megapixels:=1.0 steps:=20 prompt="$OBJECT in $SCENE, fp8, photo" \
    seed:=$((R + 630))
  s=$(run_get "$OUT" sdxl-fp8-float .status)
  expect sdxl-fp8-float "$([ "$s" = completed ] && echo PASS || echo FAIL)" "sm$SM run $(cat sdxl-fp8-float.number 2>/dev/null) $s \
$(run_get "$OUT" sdxl-fp8-float '(.reason.code // "") + " " + (.reason.message // "")' | head -c 300)"
fi
