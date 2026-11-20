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
h3_group h3-u2-finetune 100 prompt="<$NAME> $MOTION" seed:=$((R + 354)) duration_s:=5 model.base_model=sexgod1979/pinkcherry
# A 26 s shot in two segments, the second carrying the first's frames: its steps track the rows each attends (cr-288).
source "$HERE/suites/execution/multi-gpu/h3-carried.sh"
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
  for c in h3-u2-short h3-u2-warm h3-u2-finetune h3-second-0 h3-second-1; do [ -f "$c.json" ] && records+=("$c.json"); done
  got=$(jq -rs --rawfile smi h3-second.smi -f "$HERE/sessions/fixtures/h3-group/second.jq" "${records[@]}")
  expect h3-second "$([[ $got == PASS* ]] && echo PASS || echo FAIL)" "${got:-no record was read}"
}
h3_second
# LTX-2.5 over two GPUs: text to video and a retake of it (se-177 regression: a planned input a call leaves out).
source "$HERE/suites/execution/multi-gpu/ltx-group.sh"
