# h3-8gb: the 8 GB tier. H3 Turbo 15 s (one shot: FL2VA, PDD-8, 1344x768 with audio, int8) on one real 8 GB card must
# complete and stay near its time. First proof: pod-session 40557852009, RTX 3070, Runtime 0.34.7 with minimax-h3 at
# packages eb0eaf63: 2020 s (dense steps ~290 s, sparse ~159 s, decode 186 s; peak driver 8.21 GB). Production
# 0.34.5 / minimax-h3 0.1.2 ran out of memory in step 1 there, and so did ComfyUI. One rental (rtx-3070), driven from a
# runner. params:
#   target      the function: "<pkg>/<fn>[@release]" (minimax-h3/generate), or "tree:<name>" for that packages_refs
#               checkout's minimax-h3/generate
#   runtime     a candidate Runtime wheel, https://…#sha256=<hex>, installed with `cozy machine update` (optional)
#   prompt      the request (each session draws a new seed)
#   max_exec_s  the run's execution bar (2400: the proof's 2020 s and some host spread)
#   traffic_gb  GB the provider bills (85)
. "$(dirname "$0")/lib.sh"
T=$(param target minimax-h3/generate) URL=$(param runtime) MAX=$(param max_exec_s 2400)
PROMPT=$(param prompt "integrated_multimodal_description: A steady tracking shot of an adult courier cycling a cargo bike through a narrow cobbled old-town street after rain, past cafe tables and a flower stall, then down a gentle slope into a sunlit square.

audio: Tires hissing on wet cobbles, a bicycle bell, cafe chatter and a distant tram.

non_diegetic_music: N/A")
if [[ $T == tree:* ]]; then
  T=$PACKAGES/${T#tree:}/minimax-h3/generate; [ -f "${T%/*}/h3.py" ] || fail "no packages checkout ${T%/*}"
fi
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}"); mkdir -p "$OUT/wheel"
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
fi
command -v ffprobe >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
jq -n --arg p "$PROMPT" --argjson s "$RANDOM$RANDOM" '{prompt: $p, seed: $s, duration_s: 15}' > "$OUT/request.json"
quote "$(param traffic_gb 85)" 3600
rent

jq -e '.status.gpus | length == 1 and .[0].memory_bytes < 9e9' "$OUT/show.json" >/dev/null || fail "not one 8 GB GPU"
[ -z "$URL" ] || step update machine update "$NAME" --runtime "$W" -y || fail "update"
step model model download minimaxai/minimax-h3@1.0.0-rc.5 --lane int8 --on "$NAME" || fail "model download"
step lora model download alibaba-pai/minimax-h3-turbo-lora@1.0.1 --on "$NAME" || fail "LoRA download"
say "pod $(cozy machine show "$NAME" --json | jq -c '{rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name}')"
step h3 run "$T" --input "$OUT/request.json" --lane int8 --gpus=1 --on "$NAME" --out "$OUT/media"
N=$(runno h3); [ -n "$N" ] || fail "the run named no number"
cozy run show "$N" --once --json > "$OUT/h3.json" 2>&1
say "$(jq -r '([.calls[] | select(.function | endswith("_piece")) | .measurements] | last) as $m |
  "run \(.number) \(.status), exec \((.execution_ms // 0) / 1000 | floor) s; steps \([$m.attribution.steps.denoise.series[]?[1] / 1000 | floor]),
  decode \(($m.attribution.stages.decode_video.total_ms // 0) / 1000 | floor) s; peak driver \(([$m.memory.driver[]?.peak_bytes] | max // 0) / 1e9 * 100 | floor / 100) GB;
  attention \([$m.observations[]? | select(.name == "attention.selected") | .value] | unique)"' "$OUT/h3.json" | tr -s ' \n' ' ')"
[ "$(jq -r .status "$OUT/h3.json")" = completed ] || fail "run $N: $(jq -r '.reason.message // .status' "$OUT/h3.json" | cut -c1-400)"
V=$(jq -r '[.output[]?.path | select(endswith(".mp4"))][0] // empty' "$OUT/h3.json")
F=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$V" 2>/dev/null)
[ "$F" = 362 ] || fail "run $N's video decodes to ${F:-no} frames, not 362"
S=$(jq '(.execution_ms // 0) / 1000 | floor' "$OUT/h3.json")
[ "$S" -le "$MAX" ] || fail "run $N took $S s, over $MAX s"
say "PASS: H3 Turbo 15 s on an 8 GB card in $S s (bar $MAX s), 362 frames"
