# Suite `h3`: minimax-h3's shot and upscale at their edges, from the package users get (or its --pins release).
# - Regenerate-2K (`upscale.model: regenerate-2k`) at 1080p, 1440p and 2160p, each against the 768p shot it starts from: its
#   layout at 112x64 (frames.py), since regenerated texture rightly differs, and that it moves as the original does (no
#   flicker: a refine window off the latent chunk grid flickers at every chunk, run 22567).
# - SeedVR2, the default model, at 2160p on the same shot, its lane the one an empty card gives.
# - A plate (a synthetic, people-free test pattern) as long as the shot, shorter, longer, at 30 fps and at 9:16: the
#   output must follow it frame by frame.
# - First and last keyframes: the shot opens and ends on them without a cut.
# - A plate and reference footage past one piece (se-086): `guide-30s` and `footage-30s`.
# `regen-2160p-15s` (about an hour on 24 GB, in three windows), `guide-30s` and `footage-30s` run only when --only names
# them. A run that breaches its memory envelope fails (analyze.py: not completed), so every cell below also asserts that.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# video_of <cell>: its tallest mp4.
video_of() {
  run_get "$OUT" "$1" '.output[]?.path' | grep -E '\.mp4$' | while read -r p; do
    echo "$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$p") $p"; done | sort -n | tail -1 | cut -d' ' -f2-
}
# judge <check> <cell> <jq condition> <frames.py mode> <reference mp4> [args]: frames.py on the cell's video.
judge() {
  local check=$1 name=$2 cond=$3 mode=$4 ref=$5 got r; shift 5
  [ -f "$name.number" ] || return 0
  got=$(video_of "$name")
  if [ -z "$got" ] || [ ! -f "$ref" ]; then expect "$check" FAIL "no video: ${got:-$name} / ${ref:-reference}"; return; fi
  r=$("${FR[@]}" "$mode" "$ref" "$got" "$@" 2>&1 | tail -1)
  expect "$check" "$(jq -r "if $cond then \"PASS\" else \"FAIL\" end" <<< "$r" 2>/dev/null || echo FAIL)" "$r"
}
ABS='if . < 0 then -. else . end'

# Regenerate-2K and SeedVR2: one 5 s shot, upscaled whole at each size.
SHOT=$(pkg minimax-h3 shot) UP=$(pkg minimax-h3 upscale)
P="$SCENE, a painted shop sign reads OPEN, slow push-in, street sounds"
SAVED=$ONLY; [[ $ONLY == *",regen-"* || $ONLY == *",seedvr2-2160p,"* ]] && ONLY="$ONLY,regen-768p,"
cell regen-768p "$SHOT" prompt="$P" seed:=$((R + 70)) duration_s:=5
ONLY=$SAVED
for r in 1080 1440 2160; do
  cell "regen-${r}p" "$UP" --file video="$(video_of regen-768p)" resolution=${r}p prompt="$P" seed:=$((R + 70)) \
    'upscale:={"model": "regenerate-2k"}'
  av "regen-${r}p" $r
  judge "regen-${r}p-frames" "regen-${r}p" ".structure_psnr >= 18 and .structure_ssim >= 0.5 and .flicker <= 2 and .lag_ms != null and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
    regen "$(video_of regen-768p)"
done
cell seedvr2-2160p "$UP" --file video="$(video_of regen-768p)" resolution=2160p seed:=$((R + 71))
av seedvr2-2160p 2160
# After the shot and Regenerate-2K, SeedVR2's lane is the one it gets on an empty card: the idle models leave for its
# call, so it never resolves for the free bytes beside them (pawoo 22998, TensorD 0.14.3: "no lane ... fits 0.3 GiB
# free ... streaming the rest" on a 24 GB card it then had to itself; holds from #2319's TensorD).
if [ -f seedvr2-2160p.number ]; then
  w=$(grep -o 'no lane of [^"]*' "$OUT/seedvr2-2160p.ndjson" 2>/dev/null | head -1)
  expect seedvr2-lane "$([ -z "$w" ] && echo PASS || echo FAIL)" "${w:-resolved for the card, nothing streamed}"
fi
if [[ $ONLY == *",regen-2160p-15s,"* ]]; then
  P15="$SCENE at night, rain on the windows, a neon sign reads CAFE, rain and footsteps"
  SAVED=$ONLY; ONLY="$ONLY,regen-768p-15s,"
  cell regen-768p-15s "$SHOT" prompt="$P15" seed:=$((R + 72)) duration_s:=15
  ONLY=$SAVED
  cell regen-2160p-15s "$UP" --file video="$(video_of regen-768p-15s)" resolution=2160p prompt="$P15" seed:=$((R + 72)) \
    'upscale:={"model": "regenerate-2k"}'
  av regen-2160p-15s 2160
  judge regen-2160p-15s-frames regen-2160p-15s ".structure_psnr >= 18 and .structure_ssim >= 0.5 and .flicker <= 2 and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
    regen "$(video_of regen-768p-15s)"
fi

# Plates: testsrc2 (colour bars, a moving sweep and a running timecode), beside a picture of its first frame.
pattern() {  # pattern <file> <WxH> <fps> <seconds>
  nice -n 19 ffmpeg -v error -y -f lavfi -i "testsrc2=size=$2:rate=$3:duration=$4" -c:v libx264 -pix_fmt yuv420p "$1"
}
pattern guide-5s.mp4 1344x768 24 5; pattern guide-3s.mp4 1344x768 24 3; pattern guide-8s.mp4 1344x768 24 8
pattern guide-30fps.mp4 1344x768 30 5; pattern guide-9x16.mp4 1080x1920 24 5
for f in guide-5s guide-9x16; do nice -n 19 ffmpeg -v error -y -i "$f.mp4" -frames:v 1 "$f.png"; done
GUIDE_PROMPT=$'video_editing:\n\nsubject_definitions:\n<Pattern> is the background in <Picture 1>.\n<Video 1> is the source video for the target video edit.\n\nsummary:\n[video editing]\nThe target video is <Video 1>, unchanged.\n\nretention_analysis:\n<Video 1> (entire target timeline): fully_preserved - its motion, framing, timing and composition are kept.\n\ndetailed_description:\nA synthetic broadcast test pattern fills the frame: colour bars, a moving gradient sweep, a turning marker and a running timecode on a dark field. The camera is static and no person appears.\n'
guide_cell() {  # guide_cell <cell> <clip> <picture> <seed>
  cell "$1" "$SHOT" prompt="$GUIDE_PROMPT" duration_s:=5 seed:=$4 \
    assets:="[{\"name\": \"Pattern\", \"kind\": \"environment\", \"image\": \"$OUT/$3\"}]" plate:="{\"video\": \"$OUT/$2\"}"
}
guide_cell guide-equal guide-5s.mp4 guide-5s.png $((R + 73))
judge guide-equal guide-equal '.within_1 >= 0.8' guide "$OUT/guide-5s.mp4"
# Shorter: the guided 3 s follow the clip, the rest holds together. Production's spread over 72 guided frames is 0.792-0.833
# within 1 frame (gates 40991023097, 41012561523, 41153513878; seed 696197123 gives 0.792 on Runtime 0.36.7 and 0.36.8 alike,
# session 41162474167), so the bar sits 3 frames under it.
guide_cell guide-short guide-3s.mp4 guide-5s.png $((R + 74))
judge guide-short guide-short '.within_1 >= 0.75 and .coherence_after >= 0.5' guide "$OUT/guide-3s.mp4" --guided 3
# Longer: cut to the render's length, which a clip as long as the render gives (else a 5 s render's 124 frames).
guide_cell guide-long guide-8s.mp4 guide-5s.png $((R + 75))
EQ=$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$(video_of guide-equal)" 2>/dev/null || true)
judge guide-long guide-long "(.frames - ${EQ:-124} | $ABS) <= 1 and .within_1 >= 0.8" guide "$OUT/guide-8s.mp4"
# 30 fps: the clip lands on H3's 24-fps clock by timestamp, so the output matches the source retimed to 24 fps.
guide_cell guide-30fps guide-30fps.mp4 guide-5s.png $((R + 76))
judge guide-30fps guide-30fps '.within_1 >= 0.8' guide "$OUT/guide-30fps.mp4" --fps24
# 9:16: the render takes the clip's canvas, 768x1344 at 768p.
guide_cell guide-aspect guide-9x16.mp4 guide-9x16.png $((R + 77))
judge guide-aspect guide-aspect '.width == 768 and .height == 1344 and .within_1 >= 0.8' guide "$OUT/guide-9x16.mp4"
# span_seams <check> <cell>: frames.py seams at the piece boundaries its record logs (`boundaries_s`): the change there
# under 3x the shot's median (the flicker rule of #2322), and its consecutive SSIM no lower than 0.9x the shot's own away
# from the boundaries (or 0.6): a fast shot is unalike frame to frame everywhere.
span_seams() {
  [ -f "$2.number" ] && [ "$(run_get "$OUT" "$2" .status)" = completed ] || return 0
  local b r
  b=$("$CLI" run show "$(cat "$2.number")" --once 2>/dev/null | grep -o 'boundaries_s=[0-9.,]*' | head -1 | cut -d= -f2)
  r=$([ -n "$b" ] && "${FR[@]}" seams "$(video_of "$2")" --at "$b" 2>&1 | tail -1)
  expect "$1" "$(jq -r 'if (.boundaries | length) >= 2 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "boundaries ${b:-unread}: $r"
}
# Past one piece (se-086): 30 s in three pieces (15/9/6 s), each reading its own window of the clip, 1:1 with the shot,
# its carried head included. A plate as long as the shot is followed frame by frame across both edges (a piece reading
# the clip from its start again scores about 0.5); reference footage's windows are the ones its pieces' spans give.
B30='[{"at": 0, "prompt": "The pattern runs."}, {"at": 8, "prompt": "The sweep keeps turning."}, {"at": 19, "prompt": "The timecode keeps counting."}]'
PATTERN_ASSET="[{\"name\": \"Pattern\", \"kind\": \"environment\", \"image\": \"$OUT/guide-5s.png\"}]"
if [[ $ONLY == *",guide-30s,"* ]]; then
  pattern guide-30s.mp4 1344x768 24 30
  cell guide-30s "$SHOT" prompt="$GUIDE_PROMPT" duration_s:=30 beats:="$B30" seed:=$((R + 96)) assets:="$PATTERN_ASSET" \
    plate:="{\"video\": \"$OUT/guide-30s.mp4\"}"
  judge guide-30s guide-30s '.within_1 >= 0.8' guide "$OUT/guide-30s.mp4"
  span_seams guide-30s-seams guide-30s
fi
if [[ $ONLY == *",footage-30s,"* ]]; then
  pattern footage-30s.mp4 1280x720 30 30
  cell footage-30s "$SHOT" duration_s:=30 beats:="$B30" seed:=$((R + 97)) assets:="$PATTERN_ASSET" \
    prompt="<Sweep> gives the motion and timing: a synthetic broadcast test pattern, colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears." \
    reference_footage:="[{\"name\": \"Sweep\", \"video\": \"$OUT/footage-30s.mp4\"}]"
  if [ -f footage-30s.number ]; then
    s=$("$CLI" run show "$(cat footage-30s.number)" --once 2>/dev/null)
    b=$(grep -o 'boundaries_s=[0-9.,]*' <<< "$s" | head -1 | cut -d= -f2)
    w=$(grep -o 'clip_windows=[^ ]*' <<< "$s" | head -1 | cut -d= -f2)
    want=$(awk -v b="$b" 'BEGIN { n = split(b, e, ","); printf "Sweep@0"; for (i = 1; i <= n; i++) printf ";Sweep@%d", int(e[i] * 24 + 0.5) - 22 }')
    expect footage-30s-windows "$([ -n "$b" ] && [ "$w" = "$want" ] && echo PASS || echo FAIL)" "clip_windows ${w:-unread}, want $want"
  fi
  span_seams footage-30s-seams footage-30s
fi
# Keyframes: a shot from the pattern's first frame to its last (FL2VA), each end on its keyframe, with no cut onto it.
nice -n 19 ffmpeg -v error -y -sseof -0.1 -i guide-5s.mp4 -frames:v 1 guide-5s-last.png
cell keyframes "$SHOT" prompt="A synthetic broadcast test pattern: colour bars, a moving gradient sweep and a running timecode." \
  duration_s:=5 seed:=$((R + 78)) keyframes:="{\"first\": \"$OUT/guide-5s.png\", \"last\": \"$OUT/guide-5s-last.png\"}"
judge keyframes-first keyframes '.ssim >= 0.9 and .ssim > .other' still "$OUT/guide-5s.png" first
judge keyframes-last keyframes '.ssim >= 0.9 and .ssim > .other' still "$OUT/guide-5s-last.png" last

# A fine-tune and a LoRA that acts on text-only shots, through shot (0.2.x: `model.base_model` and `--lora` serve every piece), each against a same-seed
# control on the official base: coherent (not grey: the old converter bug; not falling apart) and visibly different.
# The 20 s pair is two pieces (past 15 s a shot needs beats): the LoRA must reach both halves, its record must name it on every piece, and no seam.
VP="$CHAR $MOTION at golden hour, a slow orbit, ambient sound" LORA=jojocodex/minimax-h3-spatial-physics
SAVED=$ONLY; [[ $ONLY == *",h3-checkpoint,"* || $ONLY == *",h3-lora,"* ]] && ONLY="$ONLY,h3-base,"
[[ $ONLY == *",h3-lora-20s,"* ]] && ONLY="$ONLY,h3-base-20s,"
cell h3-base "$SHOT" prompt="$VP" seed:=$((R + 80)) duration_s:=5
cell h3-base-20s "$SHOT" prompt="$VP" seed:=$((R + 81)) duration_s:=20 beats:="[{\"at\": 0, \"prompt\": \"They walk toward the camera.\"}, {\"at\": 10, \"prompt\": \"They turn and look back over one shoulder.\"}]"
ONLY=$SAVED
cell h3-checkpoint "$SHOT" prompt="$VP" seed:=$((R + 80)) duration_s:=5 model.base_model=warmbloodaban/minimax-h3-singularity@0.1.1
judge h3-checkpoint-frames h3-checkpoint '.contrast >= 0.5 * .control_contrast and .coherence >= 0.6 and .ssim < 0.9' \
  variant "$(video_of h3-base)"
cell h3-lora "$SHOT" prompt="$VP" seed:=$((R + 80)) duration_s:=5 --lora "$LORA,0.8"
judge h3-lora-frames h3-lora '.contrast >= 0.5 * .control_contrast and .coherence >= 0.6 and .ssim < 0.9' variant "$(video_of h3-base)"
cell h3-lora-20s "$SHOT" prompt="$VP" seed:=$((R + 81)) duration_s:=20 beats:="[{\"at\": 0, \"prompt\": \"They walk toward the camera.\"}, {\"at\": 10, \"prompt\": \"They turn and look back over one shoulder.\"}]" --lora "$LORA,0.8"
judge h3-lora-20s-frames h3-lora-20s '.contrast >= 0.5 * .control_contrast and .ssim_first < 0.9 and .ssim_second < 0.9' \
  variant "$(video_of h3-base-20s)"
# The seam is judged at the piece boundary (its record's `boundaries_s`), as h3-shot-40s judges its own: a camera move at
# a beat inside a piece is content (0.2.1 and 0.2.2 make the same jump at the 10 s beat from one seed, runs 24512/24340).
if [ -f h3-lora-20s.number ] && [ "$(run_get "$OUT" h3-lora-20s .status)" = completed ]; then
  b=$("$CLI" run show "$(cat h3-lora-20s.number)" --once 2>/dev/null | grep -o 'boundaries_s=[0-9.,]*' | head -1 | cut -d= -f2)
  r=$([ -n "$b" ] && "${FR[@]}" seams "$(video_of h3-lora-20s)" --at "$b" 2>&1 | tail -1)
  expect h3-lora-20s-seams "$(jq -r 'if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "boundaries ${b:-unread}: $r"
fi
# named <check> <cell> <min calls>: the run record names the LoRA (a model adapter) in at least that many of its
# generate or piece calls (0.2.x's shot runs one call per piece).
named() {
  [ -f "$2.number" ] || return 0
  local k; k=$(jq --arg l "$LORA" '[.calls[]? | select(.function | test("^generate|_piece$")) | select(tostring | contains($l))] | length' "$OUT/$2.json" 2>/dev/null)
  expect "$1" "$([ "${k:-0}" -ge "$3" ] && echo PASS || echo FAIL)" "$LORA named in ${k:-0} generate call(s), want >= $3"
}
named h3-lora-named h3-lora 1
named h3-lora-20s-named h3-lora-20s 2

# A reference shot, then a text-only render on the next call: the second must get the GPU, never waiting behind the
# first's prefetch (TensorD 0.14.6: lily run 22344 then 22363 waited until cancelled; fixed by cozy 0.14.8, #2316). On
# 0.1.x's functions, pinned, as production served them before 0.2.x.
cell h3-lease-ref minimax-h3/ref2va_turbo@0.1.19 references:="[{\"name\": \"Skater\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  seed:=$((R + 90)) duration_s:=15 resolution=768p prompt="<Skater> carves a long turn across an empty plaza at dusk, wheels on concrete"
cell h3-lease-next minimax-h3/fl2va_turbo@0.1.19 prompt="$SCENE, slow push-in, ambient sound" seed:=$((R + 91)) duration_s:=5 resolution=1080p
if [ -f h3-lease-next.number ]; then
  w=$(grep -c 'held by another run' "$OUT/h3-lease-next.ndjson" 2>/dev/null); s=$(run_get "$OUT" h3-lease-next .status)
  expect h3-lease "$([ "$s" = completed ] && [ "${w:-0}" = 0 ] && echo PASS || echo FAIL)" "next call $s, ${w:-0} waits behind another run"
fi

# A reference shot, then at once a text-only one, three times (cl-697). After H3 the host rule finds the host short as
# the next call starts and ends the idle executor (memory log `end_idle`, then `host_short ended gpu`) while that call's
# GPU child is placed: on 0.15.0 the child waited behind nothing until cancelled (jitae 23697 and 23759, 2 of 3 such
# transitions). Each executor the host rule ends during a text-only call must be followed by a placement within 30 s;
# the verdict counts how many transitions did that. An SDXL pod cannot stand in: its calls' host growth never reaches
# the host rule, so no squeeze makes it end an idle executor.
SWITCH=("carves a long turn across an empty plaza at dusk, wheels on concrete" "rolls down a ramp into a sunlit car park"
  "glides past shuttered shops under a light rain")
for i in 1 2 3; do
  cell "h3-switch-ref-$i" "$SHOT" assets:="[{\"name\": \"Skater\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
    prompt="<Skater> ${SWITCH[$((i - 1))]}" seed:=$((R + 100 + i)) duration_s:=5
  cell "h3-switch-text-$i" "$SHOT" prompt="$SCENE, take $i, slow push-in, ambient sound" seed:=$((R + 110 + i)) duration_s:=5
done
if [ -f h3-switch-text-1.number ]; then
  "$CLI" machine logs "$M" memory -n 0 > h3-switch.memory.log 2>&1
  calls=$(for i in 1 2 3; do [ -f "h3-switch-text-$i.number" ] && jq -r '"\(.status) \((.accepted_at_ms // 0) / 1000) \((.settled_at_ms // .finished_at_ms // 0) / 1000)"' "$OUT/h3-switch-text-$i.json"; done | paste -sd';')
  r=$(awk -v calls="$calls" '
    BEGIN { n = split(calls, c, ";"); for (i = 1; i <= n; i++) { split(c[i], f, " "); if (f[1] != "completed") bad++; a[i] = f[2]; s[i] = f[3] } }
    /"event":"end_idle"/ { e = $1 }
    /"event":"host_short"/ && /"ended":"gpu"/ && e && $1 - e < 10 { for (i = 1; i <= n; i++) if ($1 >= a[i] - 2 && $1 <= s[i]) { t[++k] = $1; break } }
    /"event":"place"/ { for (j = 1; j <= k; j++) if (!p[j] && $1 >= t[j]) p[j] = $1 }
    END { for (j = 1; j <= k; j++) { d = p[j] ? p[j] - t[j] : 999; if (d > 30) late++; if (d > w) w = d }
      printf "%d not completed, %d host_short-ended transitions, %d placed later than 30 s (worst %.1f s)", bad, k, late, w }' h3-switch.memory.log)
  expect h3-switch "$(grep -q '^0 not completed, [0-9]* host_short-ended transitions, 0 placed' <<< "$r" && echo PASS || echo FAIL)" "$r"
fi

# The long shot projects depend on (xs-136; it replaces the RC's h3-shot, folding in its length and A/V checks): 40 s in three pieces (beats at 0/13/27), one character picture, Turbo. Its timing
# is recorded: per piece, s/step, the whole-shot decode, render seconds per output second (baseline run 22816, 0.2.1 on
# Runtime 0.36.7: 1406.6 s, 35.2 s per output second). Its seams are judged by span_seams.
B40='[{"at": 0, "prompt": "<Rider> rolls out of a garage onto a wet street at dawn."}, {"at": 13, "prompt": "<Rider> weaves through a quiet market as stalls open."}, {"at": 27, "prompt": "<Rider> stops on a bridge and looks out over the river."}]'
cell h3-shot-40s "$SHOT" assets:="[{\"name\": \"Rider\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  prompt="<Rider> rides a bicycle through an old town, one continuous tracking shot, ambient sound" duration_s:=40 beats:="$B40" seed:=$((R + 95))
if [ -f h3-shot-40s.number ] && [ "$(run_get "$OUT" h3-shot-40s .status)" = completed ]; then
  av h3-shot-40s 768
  expect h3-shot-40s-length "$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$(video_of h3-shot-40s)" \
    | awk '{ print ($1 >= 39.5 && $1 <= 41) ? "PASS" : "FAIL" }')" "40 s asked"
  expect h3-shot-40s-timing PASS "$(jq -r '[.calls[] | select(.function | test("_piece$")) | .measurements.attribution as $a |
    {d: (($a.stages.denoise.total_ms // 0) / 1000), s: [$a.steps.denoise.series[]?[1] / 1000], v: (($a.stages.decode_video.total_ms // 0) / 1000)}] as $p |
    ((.execution_ms // 0) / 1000) as $x | "exec \($x | floor) s, \($x / 40 * 100 | floor / 100) s per output second (run 22816: 1406.6 s, 35.16);
    pieces denoise \([$p[].d | floor]) s, s/step \([$p[] | select(.s | length > 0) | (.s | add) / (.s | length) | . * 10 | floor / 10]), decode_video \([$p[].v | floor]) s"' "$OUT/h3-shot-40s.json" | tr -s ' \n' ' ')"
  span_seams h3-shot-40s-seams h3-shot-40s
fi

# Regenerate from a shot's latents (#762): a shot delivers its latents beside its video, and an upscale given both, with
# no prompt, reads the shot's own text back at strength 0.7 and never encodes the video (no encode_video stage).
cell latents-shot "$SHOT" prompt="A lighthouse keeper in a yellow oilskin climbs the spiral stair of a stone lighthouse, lantern in hand." \
  style="soft watercolor, muted greens" beats:='[{"at": 2, "prompt": "He reaches the lamp room and opens the shutters to the sea."}]' \
  duration_s:=5 resolution=768p seed:=$((R + 98))
if [ -f latents-shot.number ] && [ "$(run_get "$OUT" latents-shot .status)" = completed ]; then
  lv=$(video_of latents-shot); lt=$(run_get "$OUT" latents-shot '.output[]? | select(.name == "latents") | .path')
  expect latents-delivered "$([ -n "$lt" ] && [ -e "$lt" ] && echo PASS || echo FAIL)" "latents ${lt:-none}"
  if [ -n "$lt" ]; then
    cell latents-upscale "$UP" --file video="$lv" --file latents="$lt" resolution=1080p 'upscale:={"model": "regenerate-2k"}' \
      preset=balanced seed:=$((R + 99))
    if [ -f latents-upscale.number ] && [ "$(run_get "$OUT" latents-upscale .status)" = completed ]; then
      enc=$(jq '[.. | objects | select(has("stages")) | .stages | keys[]] | map(select(. == "encode_video")) | length' "$OUT/latents-upscale.json" 2>/dev/null)
      log=$("$CLI" run show "$(cat latents-upscale.number)" --once 2>/dev/null | grep -m1 'h3 regenerate')
      expect latents-upscale-path "$([ "${enc:-1}" = 0 ] && [[ $log == *strength=0.7* ]] && echo PASS || echo FAIL)" "encode_video stages ${enc:-?}; ${log:-no h3 regenerate line}"
      judge latents-upscale-frames latents-upscale ".structure_psnr >= 18 and .structure_ssim >= 0.5 and .flicker <= 2 and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" regen "$lv"
    fi
  fi
fi

# shot.preset (#751): the Skater request with `preset: fast` runs one dense step, then sparse ones, and denoises in at most
# 0.85x the same request on `balanced`, run just before it on the same machine and executor state (se-091: a control from
# hours earlier measured residency, not the preset); its frames hold against that control's.
IFS=$'\t' read -r PN _ PM _ _ <<< "$(grep -m1 $'^Skater\t' "$HERE/scenarios.tsv")"
PRESET_REQ=(assets:="[{\"name\": \"$PN\", \"kind\": \"character\", \"image\": \"$REF\"}]" prompt="<$PN> $PM" seed:=$((R + 101)) duration_s:=15)
SAVED=$ONLY; [[ $ONLY == *",h3-preset-fast,"* ]] && ONLY="$ONLY,h3-preset-balanced,"
cell h3-preset-balanced "$SHOT" "${PRESET_REQ[@]}" preset=balanced
ONLY=$SAVED
[ -f h3-preset-balanced.number ] && cell h3-preset-fast "$SHOT" "${PRESET_REQ[@]}" preset=fast
if [ -f h3-preset-fast.number ] && [ "$(run_get "$OUT" h3-preset-fast .status)" = completed ] && \
   [ "$(run_get "$OUT" h3-preset-balanced .status)" = completed ]; then
  steps=$(jq -c '[.calls[]? | .measurements.attribution.steps.denoise.series[]?[1]]' "$OUT/h3-preset-fast.json")
  df=$(jq '[.calls[]? | .measurements.attribution.stages.denoise.total_ms // 0] | add' "$OUT/h3-preset-fast.json")
  db=$(jq '[.calls[]? | .measurements.attribution.stages.denoise.total_ms // 0] | add' "$OUT/h3-preset-balanced.json")
  expect h3-preset-fast-steps "$(jq -r 'if length == 8 then (.[1:] | sort | .[length/2|floor]) as $m | ([.[] | select(. >= 1.6 * $m)] | length) as $k | if $k == 1 and .[0] >= 1.6 * $m then "PASS" else "FAIL" end else "FAIL" end' <<< "$steps")" "steps ${steps}"
  expect h3-preset-fast-time "$(awk -v f="$df" -v b="$db" 'BEGIN { print (b > 0 && f <= 0.85 * b) ? "PASS" : "FAIL" }')" "denoise fast ${df} ms vs balanced (run just before) ${db} ms"
  judge h3-preset-fast-frames h3-preset-fast '.contrast >= 0.5 * .control_contrast and .coherence >= 0.6' variant "$(video_of h3-preset-balanced)"
fi
