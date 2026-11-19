# Suite `multi-gpu`, every release: a 5 s Turbo H3 over two GPUs, a new prompt and seed, and a chosen fine-tune fetched from
# the Hub during the call, on one card of each kernel family
# whose group takes its own attention path (`skus`). A group opens every kernel of a site's walk, where one GPU stops at its
# first: sm120's Sage3 kernels faulted every group from 0.35.5 to 0.39.5 while one GPU and the 3090 passed (corteo).
source "$HERE/suites/execution/multi-gpu/h3-group.sh"
h3_group h3-u2-short 100 prompt="<$NAME> $MOTION" seed:=$((R + 353)) duration_s:=5
# The same group again, warm (cr-268): Sol's per-length object built by the first call serves every sparse step, none
# dense (`*warm*` in h3_group). Runtime 0.45.0 ran a group's first call at a length all dense over two H100s (47001:
# sparse 0 of 2304, so h3-u2-short-sparse fails) and exchanged over NCCL on every call (-exchange).
h3_group h3-u2-warm 100 prompt="<$NAME> $SCENE" seed:=$((R + 355)) duration_s:=5
# A model the caller chose, fetched from the Hub during the call: its run carries a capability, so GPU 0's invoke holds a
# token, and the group's `run` hands it to GPU 1 (cl-935: runs 47352, 47366 and 47432 failed group_broken, credential_at_seam).
# cr-348 C, a degree key's second group call: h3-u2-short's request, a word longer and on another seed, has its keys (each
# sharded stage's first sharded call: its video's exact shape, its other tensors, H3's packed text rows among them, to the
# next power of two), and each stage runs at the degree its machine's record gives: two until a warm time at two is recorded
# (a time measured cold, a kernel building or weights read from disk, is not), then one GPU until one is, then the faster.
# After h3-u2-warm, so a key it shares makes this its third call and leaves h3-u2-warm at two GPUs.
h3_group h3-u2-again 100 prompt="<$NAME> $MOTION Slowly." seed:=$((R + 362)) duration_s:=5
h3_again() {
  [ -f h3-u2-short.json ] && [ -f h3-u2-again.json ] || return 0
  local got records=(h3-u2-short.json)
  [ -f h3-u2-warm.json ] && records+=(h3-u2-warm.json)
  got=$(jq -rs -f "$HERE/sessions/fixtures/h3-group/again.jq" "${records[@]}" h3-u2-again.json)
  expect h3-u2-again-degree "$([[ $got == PASS* ]] && echo PASS || echo FAIL)" "${got:-no record was read}"
}
h3_again
h3_group h3-u2-finetune 100 prompt="<$NAME> $MOTION" seed:=$((R + 354)) duration_s:=5 model.base_model=sexgod1979/pinkcherry
# A 26 s shot in two segments, the second carrying the first's frames: its steps track the rows each attends (cr-288).
source "$HERE/suites/execution/multi-gpu/h3-carried.sh"
# One GPU against two (xs-208, Paul: N GPUs must beat 1 on real pods before any release): the same 5 s request on GPU 0
# alone, then on both, each from a restarted machine with no derived pages, learned peaks or page cache (machines.sh
# `pod_derived`), so both start cold. Their seeds differ: the machine's call memo answers the same request again from its
# first run whatever its GPUs (--gpus is not part of a computation, by design), as 44056226690's 2-GPU twin was. So here
# two GPUs are held to being faster; that their frames are the one-GPU run's (the CPU suite's two-rank bar, at most 0.5 of
# 255 in the worst frame) waits on a way to clear the call memo (tracker cl-1156); then the twin renders here on the same seed.
one_two() {
  local g line said=() req=(references:="$H3REF" prompt="<$NAME> $MOTION One take, rendered twice." duration_s:=5)
  [ -z "$ONLY" ] || [[ $ONLY == *",h3-gpus-1,"* || $ONLY == *",h3-gpus-2,"* ]] || return 0
  for g in 1 2; do
    pod_derived "$CLI" "$M" reset > "h3-gpus-$g.derived"
    "$CLI" machine update "$M" --gpu-memory-limit off -y > "h3-gpus-$g.restart" 2>&1
    line=$(pod_derived "$CLI" "$M" settle)
    [ -n "$line" ] && pod_derived_zero "$line" || { expect "h3-gpus-$g-cold" FAIL "derived state left after the restart: ${line:-no answer over SSH}"; return 0; }
    ONLY=${ONLY:+$ONLY,h3-gpus-$g,} cell "h3-gpus-$g" "$(pkg minimax-h3 generate)" "${req[@]}" seed:=$((R + 356 + g)) --gpus=$g
    [ "$(run_get "$OUT" "h3-gpus-$g" .status)" = completed ] || { expect h3-gpus FAIL "h3-gpus-$g: $(run_get "$OUT" "h3-gpus-$g" '.status + " " + (.reason.message // "")' | head -c 200)"; return 0; }
    jq -e '(.memo.memoized // false) == false and all(.calls[]?; (.memoized // false) == false)' "h3-gpus-$g.json" > /dev/null \
      || { expect h3-gpus FAIL "h3-gpus-$g was answered from a memo: no render to compare"; return 0; }
    said+=("$g GPU: execution $(run_get "$OUT" "h3-gpus-$g" .execution_ms) ms, denoise steps after the first $(jq -c '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length | round] | first' "h3-gpus-$g.json") ms")
  done
  # A GPU held back for heat or by its host's power limit in either run (the machine's own "slowed down": ledger.py
  # `throttled`) makes the pair no measure of the build: mameshiba's GPU 1 ran 64% of an H3 call under a heat limit
  # (pod-session 44384055109). The row fails as void, to run again on another host; such times never pass or fail a build.
  local hot e1 e2 s1 s2
  hot=$(python3 -I "$HERE/ledger.py" throttled h3-gpus-1.json h3-gpus-2.json | paste -sd' ')
  if [ -n "$hot" ]; then
    expect h3-gpus-faster FAIL "void (hot host): $hot. Run this row again on another host. ${said[*]}"
    return 0
  fi
  e1=$(run_get "$OUT" h3-gpus-1 .execution_ms) e2=$(run_get "$OUT" h3-gpus-2 .execution_ms)
  s1=$(jq '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length] | first // 0' h3-gpus-1.json)
  s2=$(jq '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length] | first // 0' h3-gpus-2.json)
  expect h3-gpus-faster "$(awk -v e1="$e1" -v e2="$e2" -v s1="$s1" -v s2="$s2" 'BEGIN { print (e2 > 0 && e2 < e1 && s2 > 0 && s2 < s1) ? "PASS" : "FAIL" }')" \
    "${said[*]}"
}
one_two
# h3-second (cr-326; sessions/h3-group.sh's `second`, on the gate's own pod): while one 1-GPU run holds GPU 0, another runs
# on the second GPU alone (sealed CUDA_VISIBLE_DEVICES=1), where torch's device 0 is NVML's 1. Its record must name that
# GPU, and every GPU row of this gate's H3 runs the name, power limit and MIG mode `nvidia-smi -i <its uuid>` reads. A
# Runtime that asked NVML by torch's ordinal recorded GPU 0's for it.
h3_second() {
  [ -z "$ONLY" ] || [[ $ONLY == *",h3-second,"* ]] || return 0
  local c n got records=()
  "$CLI" machine update "$M" --gpu-memory-limit off -y > h3-second.restart 2>&1
  for c in h3-second-0 h3-second-1; do
    one_beat prompt="$MOTION A short take, ${c##*-}." seed:=$((R + 360 + ${c##*-})) duration_s:=1
    n=$(run_submit "$OUT" "$c" run "$(pkg minimax-h3 generate)" --on "$M" "${BEAT[@]}" --gpus=1)
    [ -n "$n" ] || { expect h3-second FAIL "$c made no run: $(tail -c 300 "$c.err" 2> /dev/null)"; return 0; }
    [ "$c" = h3-second-1 ] || run_wait "$OUT" "$c" in_progress || { expect h3-second FAIL "$c never started: $(tail -c 300 "$c.err" 2> /dev/null)"; return 0; }
  done
  for c in h3-second-0 h3-second-1; do
    run_wait "$OUT" "$c"
    [ "$(run_get "$OUT" "$c" .status)" = completed ] ||
      { expect h3-second FAIL "$c $(run_get "$OUT" "$c" '.status + " " + (.reason.message // "")' | head -c 300)"; return 0; }
  done
  "$CLI" machine ssh "$M" "nvidia-smi --query-gpu=index,uuid,name,power.limit,mig.mode.current --format=csv,noheader,nounits" \
    < /dev/null 2> /dev/null | grep -E '^[0-9]+, ' > h3-second.smi
  [ -s h3-second.smi ] || { expect h3-second FAIL "no nvidia-smi answer from $M"; return 0; }
  for c in h3-u2-short h3-u2-warm h3-u2-finetune h3-gpus-1 h3-gpus-2 h3-second-0 h3-second-1; do [ -f "$c.json" ] && records+=("$c.json"); done
  got=$(jq -rs --rawfile smi h3-second.smi -f "$HERE/sessions/fixtures/h3-group/second.jq" "${records[@]}")
  expect h3-second "$([[ $got == PASS* ]] && echo PASS || echo FAIL)" "${got:-no record was read}"
}
h3_second
# xs-199, cr-348 (Paul: "N GPUs must beat 1 on real pods before any release"): LTX-2.5 5 s 1080p on two GPUs is no slower than
# on one, within the ledger's noise (ledger.py NOISE, Paul 11-15: within 5% is a tie), on every two-GPU request (coordinator
# 11-19). Gate 44411764104 on 2x rtx-5090 took 44.2 s on two GPUs against 33.1 s on one: every GPU ran every stage, and the
# base stage's steps are host-bound. Each side warm, every run on its own seed (the call memo). A Runtime with cr-348 C measures
# each sharded stage at one GPU first (GPU 0 alone), tries two once where the link and paging allow, and keeps the faster; so two
# GPUs run until one of their runs after the first measures nothing, the fifth at the latest. Each of them is judged: the first
# against one GPU's first, each without its start-up (fixtures/ltx-group/no-slower.jq), every later one against the warm run.
ltx_one_two() {
  [ -z "$ONLY" ] || [[ $ONLY == *",ltx-gpus-1,"* || $ONLY == *",ltx-gpus-2,"* ]] || return 0
  local c i hot noise got runs=()
  local req=("prompt=A hot air balloon drifts over terraced rice fields in morning mist; the camera rises with it as the burner flares. Sound: the roar of the burner, birdsong, wind."
    duration_s:=5 resolution=1080p aspect_ratio=16:9 audio:=true --lane int8)
  local rows='[.measurements, .calls[]?.measurements] | map(select(. != null) | .execution.transport.stages // {}) | add // {}'
  ltx_run() {  # <cell> <seed> <gpus>: completed and computed, else the row fails
    ONLY=${ONLY:+$ONLY,$1,} cell "$1" "$(pkg ltx-2.5 generate)" "${req[@]}" seed:="$2" --gpus="$3"
    [ "$(run_get "$OUT" "$1" .status)" = completed ] ||
      { expect ltx-gpus-no-slower FAIL "$1: $(run_get "$OUT" "$1" '.status + " " + (.reason.message // "")' | head -c 200)"; return 1; }
    jq -e '(.memo.memoized // false) == false and all(.calls[]?; (.memoized // false) == false)' "$1.json" > /dev/null ||
      { expect ltx-gpus-no-slower FAIL "$1 was answered from a memo: nothing to compare"; return 1; }
  }
  ltx_run ltx-gpus-1-warm $((R + 370)) 1 && ltx_run ltx-gpus-1 $((R + 371)) 1 || return 0
  for i in 1 2 3 4 5; do
    c=ltx-gpus-2-$i
    ltx_run "$c" $((R + 371 + i)) 2 || return 0
    runs+=("$c.json")
    [ "$i" -gt 1 ] && jq -e "$rows | all(.[]; .steady_ms == null)" "$c.json" > /dev/null && break
  done
  jq -e "$rows | all(.[]; .steady_ms == null)" "$c.json" > /dev/null ||
    { expect ltx-gpus-no-slower FAIL "two GPUs still measured a degree on their run $i: $(jq -c "$rows" "$c.json")"; return 0; }
  # A GPU held back for heat or by its host's power limit (ledger.py `throttled`) voids the row, as h3-gpus-faster.
  hot=$(python3 -I "$HERE/ledger.py" throttled ltx-gpus-1-warm.json ltx-gpus-1.json "${runs[@]}" | paste -sd' ')
  [ -z "$hot" ] || { expect ltx-gpus-no-slower FAIL "void (hot host): $hot. Run this row again on another host."; return 0; }
  noise=$(python3 -I -c 'import runpy, sys; print(runpy.run_path(sys.argv[1])["NOISE"])' "$HERE/ledger.py")
  got=$(jq -rs --argjson n "$noise" -f "$HERE/sessions/fixtures/ltx-group/no-slower.jq" ltx-gpus-1-warm.json ltx-gpus-1.json "${runs[@]}")
  expect ltx-gpus-no-slower "$([[ $got == PASS* ]] && echo PASS || echo FAIL)" \
    "${got:-no record was read}; stage degrees on $c: $(jq -c "$rows | with_entries(select(.value.degree) | .value |= .degree)" "$c.json")"
}
ltx_one_two
