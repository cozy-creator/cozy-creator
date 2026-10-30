# Suite `h3`: minimax-h3's routes at their edges, from the package users get (or its --pins release).
# - Regenerate in one call at 1080p, 1440p and 2160p, each against the 768p render it starts from (same request).
# - SeedVR2, the default method, at 2160p.
# - A guide clip (`guide: true`) as long as the render, shorter, longer, at 30 fps and at 9:16: a synthetic,
#   people-free test pattern the output must follow frame by frame.
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

# Regenerate: one render, at 768p and through the latent upscaler and refine at each size.
REGEN=(prompt="$SCENE, a painted shop sign reads OPEN, slow push-in, street sounds" seed:=$((R + 70)) duration_s:=5)
SAVED=$ONLY; [[ $ONLY == *",regen-"* ]] && ONLY="$ONLY,regen-768p,"
cell regen-768p "$(pkg minimax-h3 fl2va_turbo)" "${REGEN[@]}" resolution=768p
ONLY=$SAVED
for r in 1080 1440 2160; do
  cell "regen-${r}p" "$(pkg minimax-h3 fl2va_turbo)" "${REGEN[@]}" resolution=${r}p 'upscale:={"method":"regenerate"}'
  av "regen-${r}p" $r
  judge "regen-${r}p-frames" "regen-${r}p" ".psnr >= 18 and .ssim >= 0.5 and .lag_ms != null and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
    regen "$(video_of regen-768p)"
done
cell seedvr2-2160p "$(pkg minimax-h3 fl2va_turbo)" prompt="$CHAR $MOTION at dusk, wind and distant traffic" seed:=$((R + 71)) \
  duration_s:=5 resolution=2160p
av seedvr2-2160p 2160
if [[ $ONLY == *",regen-2160p-15s,"* ]]; then
  REGEN15=(prompt="$SCENE at night, rain on the windows, a neon sign reads CAFE, rain and footsteps" seed:=$((R + 72)) duration_s:=15)
  SAVED=$ONLY; ONLY="$ONLY,regen-768p-15s,"
  cell regen-768p-15s "$(pkg minimax-h3 fl2va_turbo)" "${REGEN15[@]}" resolution=768p
  ONLY=$SAVED
  cell regen-2160p-15s "$(pkg minimax-h3 fl2va_turbo)" "${REGEN15[@]}" resolution=2160p 'upscale:={"method":"regenerate"}'
  av regen-2160p-15s 2160
  judge regen-2160p-15s-frames regen-2160p-15s ".psnr >= 18 and .ssim >= 0.5 and (.lag_ms | $ABS) <= 10 and .corr >= 0.95" \
    regen "$(video_of regen-768p-15s)"
fi

# Guide clips: testsrc2 (colour bars, a moving sweep and a running timecode), beside a picture of its first frame.
pattern() {  # pattern <file> <WxH> <fps> <seconds>
  nice -n 19 ffmpeg -v error -y -f lavfi -i "testsrc2=size=$2:rate=$3:duration=$4" -c:v libx264 -pix_fmt yuv420p "$1"
}
pattern guide-5s.mp4 1344x768 24 5; pattern guide-3s.mp4 1344x768 24 3; pattern guide-8s.mp4 1344x768 24 8
pattern guide-30fps.mp4 1344x768 30 5; pattern guide-9x16.mp4 1080x1920 24 5
for f in guide-5s guide-9x16; do nice -n 19 ffmpeg -v error -y -i "$f.mp4" -frames:v 1 "$f.png"; done
GUIDE_PROMPT=$'video_editing:\n\nsubject_definitions:\n<Pattern> is the background in <Picture 1>.\n<Video 1> is the source video for the target video edit.\n\nsummary:\n[video editing]\nThe target video is <Video 1>, unchanged.\n\nretention_analysis:\n<Video 1> (entire target timeline): fully_preserved - its motion, framing, timing and composition are kept.\n\ndetailed_description:\nA synthetic broadcast test pattern fills the frame: colour bars, a moving gradient sweep, a turning marker and a running timecode on a dark field. The camera is static and no person appears.\n'
guide_cell() {  # guide_cell <cell> <clip> <picture> <seed>
  cell "$1" "$(pkg minimax-h3 ref2va_turbo)" prompt="$GUIDE_PROMPT" duration_s:=5 seed:=$4 clean_references:=false \
    describe_references:=false references:="[{\"name\": \"Pattern\", \"kind\": \"background\", \"image\": \"$OUT/$3\"}, {\"name\": \"Source\", \"kind\": \"video\", \"video\": \"$OUT/$2\", \"guide\": true}]"
}
guide_cell guide-equal guide-5s.mp4 guide-5s.png $((R + 73))
judge guide-equal guide-equal '.within_1 >= 0.8' guide "$OUT/guide-5s.mp4"
# Shorter: the guided 3 s follow the clip, the rest holds together.
guide_cell guide-short guide-3s.mp4 guide-5s.png $((R + 74))
judge guide-short guide-short '.within_1 >= 0.8 and .coherence_after >= 0.5' guide "$OUT/guide-3s.mp4" --guided 3
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
