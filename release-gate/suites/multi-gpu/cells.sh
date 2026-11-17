# Suite `multi-gpu`, every release: a 5 s Turbo H3 over two GPUs, a new prompt and seed, and a chosen fine-tune fetched from
# the Hub during the call, on one card of each kernel family
# whose group takes its own attention path (`skus`). A group opens every kernel of a site's walk, where one GPU stops at its
# first: sm120's Sage3 kernels faulted every group from 0.35.5 to 0.39.5 while one GPU and the 3090 passed (corteo).
# qwen-replicas needs four GPUs and stays a --cells run when sharding changes, as does h3-ulysses.sh's 15 s and 30-step.
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
# One GPU against two (xs-208): the same 5 s request and seed on GPU 0 alone, then on both, each from a restarted machine
# with no memo, derived pages, learned peaks or page cache (machines.sh `pod_derived`), so neither answers from the other
# and both start cold. Paul: N GPUs must beat 1 on real pods before any release; and a group computes what one rank does,
# so its frames are its twin's: the CPU suite's two-rank cells (e2e/cpu/drive.py `twin_gap`, at most 0.5 of 255 in the
# worst frame) moved here, as a two-rank group can't run through the real TensorD on CPU. A pair over that bar is reported
# with its numbers, never given a wider one.
one_two() {
  local g line said=() req=(references:="$H3REF" prompt="<$NAME> $MOTION One take, rendered twice." seed:=$((R + 357)) duration_s:=5)
  [ -z "$ONLY" ] || [[ $ONLY == *",h3-gpus-1,"* || $ONLY == *",h3-gpus-2,"* ]] || return 0
  for g in 1 2; do
    pod_derived "$CLI" "$M" reset > "h3-gpus-$g.derived"
    "$CLI" machine update "$M" --gpu-memory-limit off -y > "h3-gpus-$g.restart" 2>&1
    line=$(pod_derived "$CLI" "$M" settle)
    [ -n "$line" ] && pod_derived_zero "$line" || { expect "h3-gpus-$g-cold" FAIL "derived state left after the restart: ${line:-no answer over SSH}"; return 0; }
    ONLY=${ONLY:+$ONLY,h3-gpus-$g,} cell "h3-gpus-$g" "$(pkg minimax-h3 generate)" "${req[@]}" --gpus=$g
    [ "$(run_get "$OUT" "h3-gpus-$g" .status)" = completed ] || { expect h3-gpus FAIL "h3-gpus-$g: $(run_get "$OUT" "h3-gpus-$g" '.status + " " + (.reason.message // "")' | head -c 200)"; return 0; }
    jq -e '(.memo.memoized // false) == false and all(.calls[]?; (.memoized // false) == false)' "h3-gpus-$g.json" > /dev/null \
      || { expect h3-gpus FAIL "h3-gpus-$g was answered from a memo: no render to compare"; return 0; }
    said+=("$g GPU: execution $(run_get "$OUT" "h3-gpus-$g" .execution_ms) ms, denoise steps after the first $(jq -c '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length | round] | first' "h3-gpus-$g.json") ms")
  done
  local one two pairing e1 e2 s1 s2
  one=$(run_output "$OUT" h3-gpus-1 mp4) two=$(run_output "$OUT" h3-gpus-2 mp4)
  pairing=$("${FR[@]}" pair "$one" "$two" 2>&1 | tail -1)
  expect h3-gpus-same "$(jq -r 'if .frames == .other_frames and .worst <= 0.5 and .audio_samples[0] == .audio_samples[1] then "PASS" else "FAIL" end' <<< "$pairing" 2>/dev/null || echo FAIL)" \
    "2 GPUs against 1, frames of 255 and audio: $pairing"
  e1=$(run_get "$OUT" h3-gpus-1 .execution_ms) e2=$(run_get "$OUT" h3-gpus-2 .execution_ms)
  s1=$(jq '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length] | first // 0' h3-gpus-1.json)
  s2=$(jq '[.calls[]? | .measurements.attribution.steps.denoise.device.series // empty | .[1:] | add / length] | first // 0' h3-gpus-2.json)
  expect h3-gpus-faster "$(awk -v e1="$e1" -v e2="$e2" -v s1="$s1" -v s2="$s2" 'BEGIN { print (e2 > 0 && e2 < e1 && s2 > 0 && s2 < s1) ? "PASS" : "FAIL" }')" \
    "${said[*]}"
}
one_two
