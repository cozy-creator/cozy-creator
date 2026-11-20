# LTX-2.5 over two GPUs (sequence parallel, --gpus=2) on a 2-GPU machine: text to video, then a retake of it, the canvas
# with every input the context-parallel plan names.
#
# se-177 regression (escaped.tsv `ltx-u2-t2v-video`): packages 39695852 (ltx-2.5 0.4.0rc2) added `audio_timestep` and
# `video_keyframes_mask` to the plan, which only a canvas passes. Diffusers' split hook read the ones a call left out by
# position, so every 2-GPU text-to-video failed at the transformer's first call ("Expected 5 arguments but got 0", then
# IndexError; pod-session 44650415423, 2x rtx-5090, ltx-2.5 0.4.0rc3). One GPU never splits, so no other cell saw it.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# ltx_group <cell> <least frames> <target> <inputs...>: held to two ranks on two GPUs: a 704p video with audio whose DiT
# call ran at degree 2 over 2 GPUs, and frames a broken shard does not make (frames.py coherent, the bounds h3_group holds).
ltx_group() {
  local n=$1 least=$2 target=$3 v ranks hv au h fr deg ng ok look; shift 3
  cell "$n" "$target" "$@" --gpus=2
  [ -f "$n.number" ] || return 0
  v=$(run_output "$OUT" "$n" mp4)
  if [ -z "$v" ]; then
    expect "$n-video" FAIL "no mp4 ($(run_get "$OUT" "$n" '.status + " " + (.reason.message // "")' | head -c 300))"
    return 0
  fi
  ranks=$(jq -r '[.calls[]?.measurements.execution // {} | select(.degree) | [.degree, (.gpus // [] | map(.uuid) | unique | length)]]
    | max_by(.[0]) // [1, 0] | @tsv' "$n.json" 2>/dev/null)
  hv=$(ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=height,nb_read_packets -of csv=p=0 "$v")
  au=$(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name,duration -of csv=p=0 "$v")
  IFS=, read -r h fr <<< "$hv"; read -r deg ng <<< "$ranks"
  ok=PASS; { [ "${h:-0}" -ge 700 ] && [ "${fr:-0}" -ge "$least" ] && [ -n "$au" ] && [ "$deg" = 2 ] && [ "$ng" = 2 ]; } || ok=FAIL
  expect "$n-video" $ok "${h}p, $fr frames, audio ${au:-none}; degree $deg on $ng GPUs"
  look=$("${FR[@]}" coherent "$v") && jq -e '.contrast >= 15 and .motion >= 0.3 and .coherence >= 0.55 and .fine <= 0.45 and .pattern <= 3.5' <<< "$look" >/dev/null && ok=PASS || ok=FAIL
  expect "$n-frames" $ok "$look"
}
ltx_group ltx-u2-t2v 121 "$(pkg ltx-2.5 generate)" prompt="$CHAR. $MOTION" seed:=$((R + 371)) duration_s:=5 resolution=720p
# The retake's canvas passes every input the plan names (per-token timesteps, the keyframe marker): its 1.5-3 s made again.
LTX_TAKE=$(run_output "$OUT" ltx-u2-t2v mp4)
if [ -n "$LTX_TAKE" ]; then
  ltx_group ltx-u2-retake 121 "$(pkg ltx-2.5 retake)" --file video="$LTX_TAKE" start_s:=1.5 end_s:=3 seed:=$((R + 372)) \
    prompt="The $NAME stops, looks straight at the camera and nods, then carries on."
fi
