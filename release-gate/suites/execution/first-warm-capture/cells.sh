# SDXL's first warm calls right after a cold one, back to back, in a new executor: the second call records the whole
# call (denoise_loop's kept program) just after the executor's first idle period. Escaped in Runtime 0.35.8 (cr-227,
# RTX 5090 pod-session 41030183599 run 13): the recording's capture failed, stayed open after its refusal, so the
# eager rest failed `device_fault` (900) and the idle host pinning after it 906. Three cycles of restart, cold, w1, w2;
# every call completes and each w1 records the whole call (a refused recording names its reason).
# Sourced by the runtime suite, or --cells.
if [ -z "$ONLY" ] || [[ $ONLY == *",first-warm,"* || $ONLY == *",sdxl,"* ]]; then
  fw=() fwbad=()
  for c in 1 2 3; do
    "$CLI" machine update "$M" --gpu-memory-limit off -y > "first-warm-restart-$c.log" 2>&1
    for t in cold w1 w2; do
      ONLY=${ONLY:+$ONLY,first-warm-$c-$t,} cell "first-warm-$c-$t" "$(pkg sdxl generate)" \
        prompt="$OBJECT in $SCENE, cycle $c $t, ink and watercolor" seed:=$((R + 400 + 10 * c + ${#t}))
      [ -f "first-warm-$c-$t.json" ] || { fwbad+=("$c-$t: no record"); continue; }
      s=$(run_get "$OUT" "first-warm-$c-$t" .status)
      fw+=("$c-$t run $(cat "first-warm-$c-$t.number") $s")
      [ "$s" = completed ] || fwbad+=("$c-$t $s $(run_get "$OUT" "first-warm-$c-$t" '.reason.code')")
      if [ "$t" = w1 ] && [ "$s" = completed ]; then
        loop=$(run_get "$OUT" "first-warm-$c-$t" '.measurements.execution.loops[0] | "\(.mode) \(.refusals | join("; "))"')
        [[ $loop == resident* && $loop != *"cannot be recorded"* ]] || fwbad+=("$c-w1 loop: ${loop:-none}")
      fi
    done
  done
  expect first-warm-capture "$([ ${#fwbad[@]} -eq 0 ] && echo PASS || echo FAIL)" \
    "${fwbad[*]:-every call completed, each w1 recorded the whole call}; ${fw[*]}"
fi
