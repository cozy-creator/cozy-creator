# SDXL's first warm calls right after a cold one, back to back, in a new executor. The cold call runs eagerly and the
# idle period after it records the whole call (denoise_loop's kept program, xs-134 F); a call arriving first preempts
# that recording and runs eagerly. Escaped in Runtime 0.35.8 (cr-227, RTX 5090 pod-session 41030183599 run 13): a
# recording's capture failed, stayed open after its refusal, so the eager rest failed `device_fault` (900) and the idle
# host pinning after it 906. Three cycles of restart, cold, w1, w2: every call completes, and w2 runs the whole call
# natively (a refused recording names its reason). Sourced by the runtime suite, or --cells.
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
      loop=$(run_get "$OUT" "first-warm-$c-$t" '.measurements.execution.loops[0] |
        "\(.mode)\(if .reused then " reused" else "" end) \(.eager // "") \(.refusals // [] | join("; "))"')
      fw+=("$c-$t run $(cat "first-warm-$c-$t.number") $s ${loop%% *}")
      [ "$s" = completed ] || fwbad+=("$c-$t $s $(run_get "$OUT" "first-warm-$c-$t" '.reason.code')")
      [ "$t" != w2 ] || [ "$s" != completed ] || [[ $loop == "resident reused"* ]] || fwbad+=("$c-w2 loop: ${loop:-none}")
    done
  done
  expect first-warm-capture "$([ ${#fwbad[@]} -eq 0 ] && echo PASS || echo FAIL)" \
    "${fwbad[*]:-every call completed, each w2 ran the whole call natively}; ${fw[*]}"
fi
