# Suite `consumer`: real consumer cards under their own memory pressure (a capped card never fills).
# - crash-report (any card): a deliberate SIGSEGV on a call thread (crash.py). The run fails `executor_crashed`, "executor
#   crashed: SIGSEGV in <frame>", its detail the crashed thread's native frames and what it was answering, and the
#   machine's log names the core it kept (cr-275: the Runtime's crash record, TensorD's reason and cores).
# - sdxl-12gb-* (a card over 8 GB): SDXL 1024 int8, cold then four warm calls back to back in a new executor: every call
#   completes. Escaped in Runtime 0.36.4-0.36.7 (pod-session 41192253041, RTX 3060): the cold call's whole-call
#   recording freed memory inside its capture (900 from cuMemFree) and was refused, its idle retry met the same 900, and
#   the idle path took that for a device fault: the executor was poisoned and every second warm call failed "did not
#   settle quiescent" (cozy-runtime #1956).
# - sdxl-fp8-float (a card over 8 GB without fp8 tensor cores, below sm89): SDXL 1024 in its fp8 lane, each fp8.rowwise
#   weight decoded to 16-bit for torch's GEMM. Escaped in RC 0.27.0 (cr-338): cr-327 put int8.convrot's conv branch in the
#   fp8 and mxfp8 decoders, which read a `rotation_size` neither carries, so every such call failed KeyError
#   'rotation_size' (run FD8-2, RTX 3080 Ti). It must complete.
# - h3-first-8gb (an 8 GB card): H3 15 s as the machine's first H3 call, with no weights step: its 52 GB land during the
#   call, as on kushina (RTX 5060), whose executor died of SIGSEGV then (run 45627, cr-275). It must complete.
GIB=$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+' || true)
if [ -z "$ONLY" ] || [[ $ONLY == *",crash-report,"* ]]; then
  run_cell "$OUT" crash-report "$HERE/suites/consumer/crash.py" --on "$M" > /dev/null 2>&1 || true
  s=$(run_get "$OUT" crash-report .status) c=$(run_get "$OUT" crash-report .reason.code)
  m=$(run_get "$OUT" crash-report .reason.message) d=$(run_get "$OUT" crash-report .reason.detail)
  core=$("$CLI" machine logs "$M" tensord -n 0 2>/dev/null | grep -o 'an executor left its core at [^ ]*' | tail -1 || true)
  ok=FAIL
  [ "$s" = failed ] && [ "$c" = executor_crashed ] && [[ $m == "The process running this crashed (SIGSEGV in "* ]] \
    && grep -q '#0 ' <<< "$d" && grep -q 'answering: ' <<< "$d" && ok=PASS
  expect crash-report "$ok" "run $(cat crash-report.number 2>/dev/null) $s $c: $m; ${core:-no core kept}; detail: $(tr '\n' '|' <<< "$d" | head -c 700)"
fi
if [ "${GIB:-0}" -gt 8 ] && { [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-12gb,"* || $ONLY == *",sdxl,"* ]]; }; then
  "$CLI" machine update "$M" --gpu-memory-limit off -y > sdxl-12gb-restart.log 2>&1
  ran=() bad=() i=0
  for t in cold w1 w2 w3 w4; do
    i=$((i + 1))
    ONLY=${ONLY:+$ONLY,sdxl-12gb-$t,} cell "sdxl-12gb-$t" "$(pkg sdxl generate)" --lane int8 megapixels:=1.0 steps:=20 \
      prompt="$OBJECT in $SCENE, $t, photo" seed:=$((R + 600 + i))
    [ -f "sdxl-12gb-$t.json" ] || { bad+=("$t: no record"); continue; }
    s=$(run_get "$OUT" "sdxl-12gb-$t" .status)
    unet=$(run_get "$OUT" "sdxl-12gb-$t" '.measurements.residency.graphs.unet | "captures \(.captures) replays \(.replays) eager \(.eager.calls)"')
    ran+=("$t run $(cat "sdxl-12gb-$t.number") $s ${unet:-no unet graphs}")
    [ "$s" = completed ] || { bad+=("$t $s $(run_get "$OUT" "sdxl-12gb-$t" '.reason.message // .reason.code // ""' | head -c 200)"); continue; }
    # A warm call captures nothing and replays every step of the unet graph the cold one captured.
    [ "$t" = cold ] || jq -e '.measurements.residency.graphs.unet | .captures == 0 and .capture_attempts == 0
      and .eager.calls == 0 and .replays > 0' "sdxl-12gb-$t.json" > /dev/null 2>&1 || bad+=("$t unet ${unet:-none}: not a replay")
  done
  expect sdxl-12gb-warm "$([ ${#bad[@]} -eq 0 ] && echo PASS || echo FAIL)" "${bad[*]:-cold and four warm calls completed, each warm call a native replay}; ${ran[*]}"
fi
SM=$(grep -oE '· sm[0-9]+ ·' machine.txt | head -1 | grep -oE '[0-9]+' || true)
if [ "${GIB:-0}" -gt 8 ] && [ "${SM:-89}" -lt 89 ] && { [ -z "$ONLY" ] || [[ $ONLY == *",sdxl-fp8-float,"* || $ONLY == *",sdxl,"* ]]; }; then
  cell sdxl-fp8-float "$(pkg sdxl generate)" --lane fp8 megapixels:=1.0 steps:=20 prompt="$OBJECT in $SCENE, fp8, photo" \
    seed:=$((R + 630))
  s=$(run_get "$OUT" sdxl-fp8-float .status)
  expect sdxl-fp8-float "$([ "$s" = completed ] && echo PASS || echo FAIL)" "sm$SM run $(cat sdxl-fp8-float.number 2>/dev/null) $s \
$(run_get "$OUT" sdxl-fp8-float '(.reason.code // "") + " " + (.reason.message // "")' | head -c 300)"
fi
if [ "${GIB:-99}" -le 8 ] && { [ -z "$ONLY" ] || [[ $ONLY == *",h3-first-8gb,"* ]]; }; then
  one_beat prompt="$MOTION Wide, steady." seed:=$((R + 620)) duration_s:=15
  run_cell "$OUT" h3-first-8gb "$(pkg minimax-h3 generate)" --on "$M" "${BEAT[@]}" > /dev/null 2>&1 || true
  s=$(run_get "$OUT" h3-first-8gb .status)
  expect h3-first-8gb "$([ "$s" = completed ] && echo PASS || echo FAIL)" "run $(cat h3-first-8gb.number 2>/dev/null) $s \
$(run_get "$OUT" h3-first-8gb '(.reason.code // "") + " " + (.reason.message // "")' | head -c 300)"
fi
