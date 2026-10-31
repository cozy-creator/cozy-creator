# Suite `h3`: minimax-h3's shot and upscale at their edges, from the package users get (or its --pins release).
# - Regenerate-2K (`upscale.model: regenerate-2k`) at 1080p, 1440p and 2160p, each against the 768p shot it starts from: its
#   layout at 112x64 (frames.py), since regenerated texture rightly differs.
# - SeedVR2, the default model, at 2160p on the same shot.
# - A plate (a synthetic, people-free test pattern) as long as the shot, shorter, longer, at 30 fps and at 9:16: the
#   output must follow it frame by frame.
# - First and last keyframes: the shot opens and ends on them without a cut.
# `regen-2160p-15s` (about an hour on 24 GB, in three windows) runs only when --only names it. A run that breaches its
# memory envelope fails (analyze.py: not completed), so every cell below also asserts that.
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
  judge "regen-${r}p-frames" "regen-${r}p" ".structure_psnr >= 18 and .structure_ssim >= 0.5 and .lag_ms != null and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
    regen "$(video_of regen-768p)"
done
cell seedvr2-2160p "$UP" --file video="$(video_of regen-768p)" resolution=2160p seed:=$((R + 71))
av seedvr2-2160p 2160
if [[ $ONLY == *",regen-2160p-15s,"* ]]; then
  P15="$SCENE at night, rain on the windows, a neon sign reads CAFE, rain and footsteps"
  SAVED=$ONLY; ONLY="$ONLY,regen-768p-15s,"
  cell regen-768p-15s "$SHOT" prompt="$P15" seed:=$((R + 72)) duration_s:=15
  ONLY=$SAVED
  cell regen-2160p-15s "$UP" --file video="$(video_of regen-768p-15s)" resolution=2160p prompt="$P15" seed:=$((R + 72)) \
    'upscale:={"model": "regenerate-2k"}'
  av regen-2160p-15s 2160
  judge regen-2160p-15s-frames regen-2160p-15s ".structure_psnr >= 18 and .structure_ssim >= 0.5 and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
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
judge h3-lora-20s-frames h3-lora-20s '.contrast >= 0.5 * .control_contrast and .seam >= 0.5 and .ssim_first < 0.9 and .ssim_second < 0.9' \
  variant "$(video_of h3-base-20s)"
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

# The long shot projects depend on (xs-136; it replaces the RC's h3-shot, folding in its length and A/V checks): 40 s in three pieces (beats at 0/13/27), one character picture, Turbo. Its timing
# is recorded: per piece, s/step, the whole-shot decode, render seconds per output second (baseline run 22816, 0.2.1 on
# Runtime 0.36.7: 1406.6 s, 35.2 s per output second). Each piece boundary (its record's `boundaries_s`) is judged with
# frames.py seams: the change there under 3x the shot's median (the flicker rule of #2322), consecutive SSIM >= 0.6.
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
  b=$("$CLI" run show "$(cat h3-shot-40s.number)" --once 2>/dev/null | grep -o 'boundaries_s=[0-9.,]*' | head -1 | cut -d= -f2)
  r=$([ -n "$b" ] && "${FR[@]}" seams "$(video_of h3-shot-40s)" --at "$b" 2>&1 | tail -1)
  expect h3-shot-40s-seams "$(jq -r 'if (.boundaries | length) >= 2 and ([.boundaries[] | .ratio <= 3 and .ssim >= 0.6] | all) then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "boundaries ${b:-unread}: $r"
fi
