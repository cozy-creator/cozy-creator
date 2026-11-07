# H3 over two GPUs (sequence parallel, --gpus=2) on a 2-GPU machine, with one character picture, as a project sends it.
H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# h3_group <cell> <least frames> <inputs...>: held to two ranks on two GPUs, a 768p video with audio, and frames a broken
# kernel's output does not make. frames.py coherent, measured: real H3 clips contrast 18-78, motion 0.8-10, coherence
# 0.66-0.98 (fast scenes lowest); grey contrast 0, frozen motion 0, full-frame noise coherence 0.18, plaid (run 28451) 0.45.
# Coherence can't see subtler garble: FA3-FP8's clip (run 1250, "garbled, mostly useless") scores 0.74 like a fast clean scene.
# `fine` does: plaid 0.125, FA3-FP8 0.0225, against at most 0.0114 over 49 clean clips across sm86-sm120, 0.0026 for a 0.74
# coherence one, and 0.0028-0.0036 for 1250's same-seed siblings. It rests on one garbled clip: a pass still needs frames viewed.
h3_group() {
  local n=$1 least=$2 v ranks hv au h fr deg ng ok look; shift 2
  cell "$n" "$(pkg minimax-h3 generate)" references:="$H3REF" "$@" --gpus=2
  [ -f "$n.json" ] || return 0
  v=$(run_output "$OUT" "$n" mp4)
  # The DiT call's execution, not the parent's: the widest call's degree and its GPUs.
  ranks=$(jq -r '[.calls[]?.measurements.execution // {} | select(.degree) | [.degree, (.gpus // [] | map(.uuid) | unique | length)]]
    | max_by(.[0]) // [1, 0] | @tsv' "$n.json" 2>/dev/null)
  if [ -z "$v" ]; then expect "$n-video" FAIL "no mp4 ($(run_get "$OUT" "$n" .status))"; return 0; fi
  hv=$(ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=height,nb_read_packets -of csv=p=0 "$v")
  au=$(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name,duration -of csv=p=0 "$v")
  IFS=, read -r h fr <<< "$hv"; read -r deg ng <<< "$ranks"
  ok=PASS; { [ "${h:-0}" -ge 700 ] && [ "${fr:-0}" -ge "$least" ] && [ -n "$au" ] && [ "$deg" = 2 ] && [ "$ng" = 2 ]; } || ok=FAIL
  expect "$n-video" $ok "${h}p, $fr frames, audio ${au:-none}; degree $deg on $ng GPUs"
  look=$("${FR[@]}" coherent "$v") && jq -e '.contrast >= 15 and .motion >= 0.3 and .coherence >= 0.55 and .fine <= 0.45 and .pattern <= 3.5' <<< "$look" >/dev/null && ok=PASS || ok=FAIL
  expect "$n-frames" $ok "$look"
}
