# Suite `consumer`: SDXL 1024 int8 on a real 12 GB card, cold then four warm calls back to back in a new executor:
# every call completes. Escaped in Runtime 0.36.4-0.36.7 (pod-session 41192253041, RTX 3060): the cold call's
# whole-call recording freed memory inside its capture (900 from cuMemFree) and was refused, its idle retry met the
# same 900, and the idle path took that for a device fault: the executor was poisoned and every second warm call
# failed "did not settle quiescent" (cozy-runtime #1956).
if [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-12gb,"* || $ONLY == *",sdxl,"* ]]; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > sdxl-12gb-restart.log 2>&1
  ran=() bad=() i=0
  for t in cold w1 w2 w3 w4; do
    i=$((i + 1))
    ONLY=${ONLY:+$ONLY,sdxl-12gb-$t,} cell "sdxl-12gb-$t" "$(pkg sdxl generate)" --lane int8 megapixels:=1.0 steps:=20 \
      prompt="$OBJECT in $SCENE, $t, photo" seed:=$((R + 600 + i))
    [ -f "sdxl-12gb-$t.json" ] || { bad+=("$t: no record"); continue; }
    s=$(run_get "$OUT" "sdxl-12gb-$t" .status)
    ran+=("$t run $(cat "sdxl-12gb-$t.number") $s")
    [ "$s" = completed ] || bad+=("$t $s $(run_get "$OUT" "sdxl-12gb-$t" '.reason.message // .reason.code // ""' | head -c 200)")
  done
  expect sdxl-12gb-warm "$([ ${#bad[@]} -eq 0 ] && echo PASS || echo FAIL)" "${bad[*]:-cold and four warm calls completed}; ${ran[*]}"
fi
