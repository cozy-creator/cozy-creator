# SDXL's first warm calls right after a cold one, back to back, in a new executor. The cold call records the whole call
# after its eager first step and runs the rest natively (denoise_loop's kept program, xs-134 F2); w1 and w2 run it
# natively from their start. Escaped twice. Runtime 0.35.8 (cr-227, RTX 5090 pod-session 41030183599 run 13): a
# recording's capture failed, stayed open after its refusal, so the eager rest failed `device_fault` (900) and the idle
# host pinning after it 906. Runtime 0.36.4 (xs-134 F2, the R1 bench's RTX 4090): the cold call ran eagerly, and w1,
# sent as it settled, met its idle recording mid-way and recorded again (4.09 s against ComfyUI's 3.25 s). Three
# cycles of restart, cold, w1, w2: every call completes, the cold call records on itself, and w1 and w2 run the
# program it kept (no recording of their own). Sourced by the runtime suite, or --cells.
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
      loop=$(run_get "$OUT" "first-warm-$c-$t" '.measurements.execution.loops[0] | (.mode
        + (if .reused then "+reused" else "" end) + (if .whole_call then "+recorded" else "" end))
        + " \(.eager // "") \(.refusals // [] | join("; "))"')
      fw+=("$c-$t run $(cat "first-warm-$c-$t.number") $s ${loop%% *}")
      [ "$s" = completed ] || { fwbad+=("$c-$t $s $(run_get "$OUT" "first-warm-$c-$t" '.reason.code')"); continue; }
      want=resident+reused
      [ "$t" = cold ] && want=resident+recorded
      [ "${loop%% *}" = "$want" ] || fwbad+=("$c-$t loop: ${loop:-none}")
    done
  done
  expect first-warm-capture "$([ ${#fwbad[@]} -eq 0 ] && echo PASS || echo FAIL)" \
    "${fwbad[*]:-every call completed, each cold call recorded on itself, each w1 and w2 ran its program}; ${fw[*]}"
fi
