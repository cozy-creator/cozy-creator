# Suite `runtime`: every first-party package's default call on this SKU (products.sh), then a restart and SDXL in a
# second executor. It proves every model still installs, loads, runs, serves its kernels and holds its numbers.
for p in "${PRODUCTS[@]}"; do product "$p"; done
# Second executor process: a GPU memory limit write restarts the machine, so every executor is new.
if [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-p2,"* || $ONLY == *",sdxl,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y >restart.log 2>&1; log "restarted $M: $(grep -E '^status' restart.log)"
fi
cell sdxl-p2 "$(pkg sdxl generate)" prompt="$OBJECT on a wooden table, product photo" seed:=$((R + 7))
# A weights output: SDXL's int8 recipe re-quantizes its newest release on the machine. Escaped in Runtime 0.33.0 (xs-105):
# every weights output failed, `open()` passing TensorFS 0.16 a `revision` it no longer takes.
if [ -z "$ONLY" ] || [[ $ONLY == *",quantize-sdxl,"* ]]; then
  q=$(pkg sdxl generate); q=${q/\/generate/}
  run_submit "$OUT" quantize-sdxl model quantize sdxl --lane int8 --package "$q" --on "$M" > /dev/null; run_wait "$OUT" quantize-sdxl
  s=$(run_get "$OUT" quantize-sdxl .status)
  expect quantize-sdxl-int8 "$([ "$s" = completed ] && echo PASS || echo FAIL)" \
    "model quantize sdxl --lane int8 --package $q: run $(cat quantize-sdxl.number 2>/dev/null) $s $(run_get "$OUT" quantize-sdxl '.reason.message' | head -c 300)"
fi
