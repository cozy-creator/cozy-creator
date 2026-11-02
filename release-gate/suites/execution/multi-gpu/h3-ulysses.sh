# H3 over two GPUs (sequence parallel, --gpus=2) on a 2-GPU machine: a 15 s Turbo call and a standard 30-step call,
# each with one character picture, as a project sends it. Sourced by gate.sh --cells;
# `only` names h3-u2-turbo, h3-u2-std. Each is held to two ranks on two GPUs and a 768p, 300+ frame video with audio.
H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
cell h3-u2-turbo "$(pkg minimax-h3 generate)" references:="$H3REF" prompt="<$NAME> $MOTION" seed:=$((R + 351)) duration_s:=15 --gpus=2
cell h3-u2-std "$(pkg minimax-h3 generate)" references:="$H3REF" prompt="<$NAME> $MOTION The camera slowly pushes in." \
  seed:=$((R + 352)) duration_s:=15 turbo:=false steps:=30 --gpus=2
for n in h3-u2-turbo h3-u2-std; do
  [ -f "$n.json" ] || continue
  v=$(run_output "$OUT" "$n" mp4)
  # The DiT call's execution, not the parent's: the widest call's degree and its GPUs.
  ranks=$(jq -r '[.calls[]?.measurements.execution // {} | select(.degree) | [.degree, (.gpus // [] | map(.uuid) | unique | length)]]
    | max_by(.[0]) // [1, 0] | @tsv' "$n.json" 2>/dev/null)
  if [ -z "$v" ]; then expect "$n-video" FAIL "no mp4 ($(run_get "$OUT" "$n" .status))"; continue; fi
  hv=$(ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=height,nb_read_packets -of csv=p=0 "$v")
  au=$(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name,duration -of csv=p=0 "$v")
  IFS=, read -r h fr <<< "$hv"; read -r deg ng <<< "$ranks"
  ok=PASS; { [ "${h:-0}" -ge 700 ] && [ "${fr:-0}" -ge 300 ] && [ -n "$au" ] && [ "$deg" = 2 ] && [ "$ng" = 2 ]; } || ok=FAIL
  expect "$n-video" $ok "${h}p, $fr frames, audio ${au:-none}; degree $deg on $ng GPUs"
done
