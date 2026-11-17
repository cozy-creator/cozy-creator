# SDXL's first warm calls right after a cold one, back to back, in a new executor. The cold call captures the unet's step
# graph and replays it; w1 and w2 capture nothing and replay every step (measurements.residency.graphs.unet; denoise_loop's
# kept program until Runtime 0.43.1, xs-134 F2). Escaped twice. Runtime 0.35.8 (cr-227, RTX 5090 pod-session 41030183599
# run 13): a recording's capture failed, stayed open after its refusal, so the eager rest failed `device_fault` (900) and
# the idle host pinning after it 906. Runtime 0.36.4 (xs-134 F2, the R1 bench's RTX 4090): the cold call ran eagerly, and
# w1, sent as it settled, met its idle recording mid-way and recorded again (4.09 s against ComfyUI's 3.25 s). Three
# cycles of restart, cold, w1, w2: every call completes, the cold call captures on itself, and w1 and w2 replay what it
# captured (no capture of their own). Sourced by the runtime suite, or --cells.
if [ -z "$ONLY" ] || [[ $ONLY == *",first-warm,"* || $ONLY == *",sdxl,"* ]]; then
  fw=() fwbad=() i=0
  for c in 1 2 3; do
    "$CLI" machine update "$M" --gpu-memory-limit off -y > "first-warm-restart-$c.log" 2>&1
    for t in cold w1 w2; do
      i=$((i + 1))
      ONLY=${ONLY:+$ONLY,first-warm-$c-$t,} cell "first-warm-$c-$t" "$(pkg sdxl generate)" \
        prompt="$OBJECT in $SCENE, cycle $c $t, ink and watercolor" seed:=$((R + 400 + i))
      [ -f "first-warm-$c-$t.json" ] || { fwbad+=("$c-$t: no record"); continue; }
      s=$(run_get "$OUT" "first-warm-$c-$t" .status)
      unet=$(run_get "$OUT" "first-warm-$c-$t" '.measurements.residency.graphs.unet
        | "captures \(.captures) replays \(.replays) eager \(.eager.calls)"')
      fw+=("$c-$t run $(cat "first-warm-$c-$t.number") $s ${unet:-no unet graphs}")
      [ "$s" = completed ] || { fwbad+=("$c-$t $s $(run_get "$OUT" "first-warm-$c-$t" '.reason.code')"); continue; }
      want='.captures == 0 and .capture_attempts == 0 and .eager.calls == 0 and .replays > 0'
      [ "$t" = cold ] && want='.captures >= 1 and .replays > 0'
      jq -e ".measurements.residency.graphs.unet | $want" "first-warm-$c-$t.json" > /dev/null 2>&1 \
        || fwbad+=("$c-$t unet: ${unet:-no graphs}")
    done
  done
  expect first-warm-capture "$([ ${#fwbad[@]} -eq 0 ] && echo PASS || echo FAIL)" \
    "${fwbad[*]:-every call completed, each cold call captured the unet, each w1 and w2 replayed every step}; ${fw[*]}"
fi
