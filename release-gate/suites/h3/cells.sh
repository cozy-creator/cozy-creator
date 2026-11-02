# Suite `h3`: minimax-h3's generate and upscale at their edges, from the package users get (or its --pins release).
# - Regenerate-2K (`upscale.model: regenerate-2k`) at 1080p, 1440p and 2160p, each against the 768p shot it starts from: its
#   layout at 112x64 (frames.py), since regenerated texture rightly differs, and that it moves as the original does (no
#   flicker: a refine window off the latent chunk grid flickers at every chunk, run 22567).
# - SeedVR2, the default model, at 2160p on the same shot, its lane the one an empty card gives.
# - A plate (a synthetic, people-free test pattern) as long as the shot, shorter, longer, at 30 fps and at 9:16: the
#   output must follow it frame by frame.
# - First and last keyframes: the shot opens and ends on them without a cut.
# - A plate and reference footage past one piece (se-086): `guide-30s` and `footage-30s`.
# - A video continued from its latents (`continue_from`, xs-139), and the full model at 30 steps.
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
SHOT=$(pkg minimax-h3 generate) UP=$(pkg minimax-h3 upscale)
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
    references:="[{\"name\": \"Pattern\", \"kind\": \"environment\", \"image\": \"$OUT/$3\"}]" plate:="{\"video\": \"$OUT/$2\"}"
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
# Continued-piece text (se-089, #787): generate must write 0.2.8's piece prompts byte for byte, compared by the sha256
# each piece's `h3 piece prompt` line logs against 0.2.8's gate runs (seed-independent). The digests were computed offline
# by each tree's own function up to its piece calls: 0.2.8's reproduces the three runs, and 0.3.0's (xs-139) matches them.
same_text() {  # same_text <check> <cell> <run it must equal> <its piece digests>
  [ -f "$2.number" ] || return 0
  local got
  got=$("$CLI" run show "$(cat "$2.number")" --once 2>/dev/null | grep -oE 'h3 piece prompt chars=[0-9]+ digest=[0-9a-f]+' \
    | sed 's/.*digest=//' | paste -sd' ')
  expect "$1" "$([ "$got" = "$4" ] && echo PASS || echo FAIL)" "pieces ${got:-none}; run $3: $4"
}
# Past one piece (se-086): 30 s in three pieces (15/9/6 s), each reading its own window of the clip, 1:1 with the shot,
# its carried head included. A plate as long as the shot is followed frame by frame across both edges (a piece reading
# the clip from its start again scores about 0.5); reference footage's windows are the ones its pieces' spans give.
B30='[{"at": 0, "prompt": "The pattern runs."}, {"at": 8, "prompt": "The sweep keeps turning."}, {"at": 19, "prompt": "The timecode keeps counting."}]'
PATTERN_REF="{\"name\": \"Pattern\", \"kind\": \"environment\", \"image\": \"$OUT/guide-5s.png\"}"
if [[ $ONLY == *",guide-30s,"* ]]; then
  pattern guide-30s.mp4 1344x768 24 30
  cell guide-30s "$SHOT" prompt="$GUIDE_PROMPT" duration_s:=30 beats:="$B30" seed:=$((R + 96)) references:="[$PATTERN_REF]" \
    plate:="{\"video\": \"$OUT/guide-30s.mp4\"}"
  judge guide-30s guide-30s '.within_1 >= 0.8' guide "$OUT/guide-30s.mp4"
  span_seams guide-30s-seams guide-30s
fi
if [[ $ONLY == *",footage-30s,"* ]]; then
  pattern footage-30s.mp4 1280x720 30 30
  cell footage-30s "$SHOT" duration_s:=30 beats:="$B30" seed:=$((R + 97)) \
    prompt="<Sweep> gives the motion and timing: a synthetic broadcast test pattern, colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears." \
    references:="[$PATTERN_REF, {\"name\": \"Sweep\", \"kind\": \"video\", \"video\": \"$OUT/footage-30s.mp4\"}]"
  if [ -f footage-30s.number ]; then
    s=$("$CLI" run show "$(cat footage-30s.number)" --once 2>/dev/null)
    b=$(grep -o 'boundaries_s=[0-9.,]*' <<< "$s" | head -1 | cut -d= -f2)
    w=$(grep -o 'clip_windows=[^ ]*' <<< "$s" | head -1 | cut -d= -f2)
    want=$(awk -v b="$b" 'BEGIN { n = split(b, e, ","); printf "Sweep@0"; for (i = 1; i <= n; i++) printf ";Sweep@%d", int(e[i] * 24 + 0.5) - 22 }')
    expect footage-30s-windows "$([ -n "$b" ] && [ "$w" = "$want" ] && echo PASS || echo FAIL)" "clip_windows ${w:-unread}, want $want"
  fi
  span_seams footage-30s-seams footage-30s
  same_text footage-30s-text footage-30s 25471 \
    "294da20429a29e84d7faf17e18aeb58d30fbcab632cc37756e4040c3ec3cce9f bf6fec18e39d992c497ac953c82732f580b0020a88a9d33e2b24bb76e1261d0a 7e8123fae5b3de6743b439a89ef02d766777d97190fdb61ee916547973088cc2"
fi
# Keyframes: a shot from the pattern's first frame to its last (FL2VA), each end on its keyframe, with no cut onto it.
nice -n 19 ffmpeg -v error -y -sseof -0.1 -i guide-5s.mp4 -frames:v 1 guide-5s-last.png
cell keyframes "$SHOT" prompt="A synthetic broadcast test pattern: colour bars, a moving gradient sweep and a running timecode." \
  duration_s:=5 seed:=$((R + 78)) --file first_frame="$OUT/guide-5s.png" --file last_frame="$OUT/guide-5s-last.png"
judge keyframes-first keyframes '.ssim >= 0.9 and .ssim > .other' still "$OUT/guide-5s.png" first
judge keyframes-last keyframes '.ssim >= 0.9 and .ssim > .other' still "$OUT/guide-5s-last.png" last

# A fine-tune and a LoRA that acts on text-only shots, through generate (`model.base_model` and `--lora` serve every piece), each against a same-seed
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
# generate or piece calls (generate runs one call per piece).
named() {
  [ -f "$2.number" ] || return 0
  local k; k=$(jq --arg l "$LORA" '[.calls[]? | select(.function | test("^generate|_piece$")) | select(tostring | contains($l))] | length' "$OUT/$2.json" 2>/dev/null)
  expect "$1" "$([ "${k:-0}" -ge "$3" ] && echo PASS || echo FAIL)" "$LORA named in ${k:-0} generate call(s), want >= $3"
}
named h3-lora-named h3-lora 1
named h3-lora-20s-named h3-lora-20s 2

# A shot that moves on from its opening (se-087): one character picture, the opening a place and an activity the beats
# leave, three pieces. A later piece that restates the opening is pulled back to it with a cut at its edge (0.2.4's
# h3-base-20s/lora-20s, 24980/24981: back in the food cart the otter had walked away from). Its frames are also looked
# at for a cut and for people the shot never had (se-089). One seed is one sample: the same request, seed and Runtime
# render a different take on another machine (se-089: 25229 and 25344 differ from piece 1), so a failure holds the
# release for a look at more seeds rather than convicting a change on its own.
BMOVE='[{"at": 0, "prompt": "<Rider> flips noodles in a wok at a harbour food stall."}, {"at": 9, "prompt": "<Rider> hands the bowl over, steps out of the stall and walks along the pier."}, {"at": 19, "prompt": "<Rider> reaches the end of the pier and looks out at the boats."}]'
cell h3-shot-moves-on "$SHOT" references:="[{\"name\": \"Rider\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  prompt="<Rider> cooks at a busy harbour food stall at dusk, steam and lanterns, ambient sound" duration_s:=30 beats:="$BMOVE" seed:=$((R + 96))
span_seams h3-shot-moves-on-seams h3-shot-moves-on
# The same shot with the cook in prose only and no picture: #787 gives it #781's neutral continuation (0.2.8 gate 2).
BTEXT='[{"at": 0, "prompt": "The cook flips noodles in a wok at a harbour food stall."}, {"at": 9, "prompt": "The cook hands the bowl over, steps out of the stall and walks along the pier."}, {"at": 19, "prompt": "The cook reaches the end of the pier and looks out at the boats."}]'
cell h3-moves-on-text "$SHOT" prompt="A young cook in a red jacket cooks at a busy harbour food stall at dusk, steam and lanterns, ambient sound" \
  duration_s:=30 beats:="$BTEXT" seed:=$((R + 130))
span_seams h3-moves-on-text-seams h3-moves-on-text
# Both are also judged by eye for a cut: one frame a second, 10 to a row.
for c in h3-shot-moves-on h3-moves-on-text; do
  [ -f "$c.number" ] && v=$(video_of "$c") && [ -n "$v" ] || continue
  nice -n 19 ffmpeg -v error -y -i "$v" -vf "fps=1,scale=224:-2,tile=10x3" -frames:v 1 "$c-sheet.png" && log "$c: judge by eye for a cut: $PWD/$c-sheet.png"
done

same_text h3-shot-moves-on-text h3-shot-moves-on 25472 \
  "f36788038ca0754f381b93faf2f69366832820109acb68e8e425a7d83ee0739f 53870fb783957f792da3bf1d4270bc450b8df157f33b9178dc0123c8cbd4e3b6 5115e226a1e731785094cca8e54961bca33cec81d879cbaaad19b86b5f15ab5e"
same_text h3-moves-on-text-text h3-moves-on-text 25500 \
  "329dba4600de1b74dbb4d63eb683dfab21ff02b9f48e3c9b1b0f81c12d70fa78 2b8e25fbb70f3e23c47671a12d75abebc5c98651b444678cc4e7d015c79ea2f5 35918e8dd2207b0aaed8ca1f27b0579e3714c9bcf041fd3a271658530fd6d383"

# A reference shot, then a text-only render on the next call: the second must get the GPU, never waiting behind the
# first's prefetch (TensorD 0.14.6: lily run 22344 then 22363 waited until cancelled; fixed by cozy 0.14.8, #2316): a
# reference video (Ref2VA) then a text-only one (FL2VA) through generate.
cell h3-lease-ref "$SHOT" references:="[{\"name\": \"Skater\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  seed:=$((R + 90)) duration_s:=15 prompt="<Skater> carves a long turn across an empty plaza at dusk, wheels on concrete"
cell h3-lease-next "$SHOT" prompt="$SCENE, slow push-in, ambient sound" seed:=$((R + 91)) duration_s:=5 resolution=1080p
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
  cell "h3-switch-ref-$i" "$SHOT" references:="[{\"name\": \"Skater\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
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
cell h3-shot-40s "$SHOT" references:="[{\"name\": \"Rider\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
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
SAVED=$ONLY; [[ $ONLY == *",h3-continue,"* ]] && ONLY="$ONLY,latents-shot,"
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
ONLY=$SAVED

# continue_from (xs-139): latents-shot's video goes on to 10 s on its own timeline, from its latents. The whole 10 s comes
# back, its head as latents-shot made it (the C2 proof's head SSIM was 0.989 at its lowest, runs 25270/25271), and the
# join holds as a piece boundary does (span_seams' bar). A continuation renders in Ref2VA, which needs a picture
# (0.3.0 refuses one without, run 25881): the lamp room as latents-shot's last frame shows it, as the place.
if [ -n "${lt:-}" ] && [ -e "$lt" ]; then
  nice -n 19 ffmpeg -v error -y -sseof -0.1 -i "$lv" -frames:v 1 "$OUT/latents-shot-last.png"
  cell h3-continue "$SHOT" prompt="A lighthouse keeper in a yellow oilskin climbs the spiral stair of <Lighthouse>, lantern in hand." \
    style="soft watercolor, muted greens" beats:='[{"at": 5, "prompt": "He lights the great lamp; its beam sweeps out over the dark sea."}]' \
    references:="[{\"name\": \"Lighthouse\", \"kind\": \"environment\", \"image\": \"$OUT/latents-shot-last.png\"}]" \
    duration_s:=10 seed:=$((R + 94)) continue_from:="{\"video\": \"$lv\", \"latents\": \"$lt\"}"
  if [ -f h3-continue.number ] && [ "$(run_get "$OUT" h3-continue .status)" = completed ]; then
    # The head is latents-shot's frames, counted (its container duration runs a little past its last frame, and a
    # trim to it took the first continued frame too: 25905's 0.899 was frame 124, the head's own lowest 0.980).
    cv=$(video_of h3-continue)
    N=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$lv")
    L=$(awk -v n="$N" 'BEGIN { printf "%.6f", n / 24 }')
    n=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$cv")
    head=$(nice -n 19 ffmpeg -v error -i "$cv" -i "$lv" -filter_complex \
      "[0:v]trim=end_frame=$N,setpts=PTS-STARTPTS[a];[1:v]trim=end_frame=$N,setpts=PTS-STARTPTS[b];[a][b]ssim=stats_file=-" -f null - 2>/dev/null \
      | grep -o 'All:[0-9.]*' | cut -d: -f2 | sort -n | head -1)
    expect h3-continue-whole "$(awk -v n="$n" -v h="$head" 'BEGIN { print (n >= 9.5 && n <= 11.5 && h >= 0.95) ? "PASS" : "FAIL" }')" \
      "${n:-?} s back for 10 s asked; head SSIM against latents-shot at its lowest ${head:-unread}"
    r=$("${FR[@]}" seams "$cv" --at "$L" 2>&1 | tail -1)
    expect h3-continue-seam "$(jq -r 'if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "join at ${L:-?} s: $r"
  fi
fi

# The full model (turbo: false) at 30 steps, the quality tier (0.2.7, #765): 5 s with a character picture, 768p with sound.
cell h3-full-30 "$SHOT" references:="[{\"name\": \"Skater\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
  prompt="<Skater> waves to a friend across a sunny skate park, a painted wall behind reads SKATE OR DIE" seed:=$((R + 93)) \
  duration_s:=5 turbo:=false steps:=30
av h3-full-30 768

# generate's preset (#751): the Skater request with `preset: fast` runs one dense step, then sparse ones, and denoises in at most
# 0.90x the same request on `balanced`, run just before it on the same machine and executor state (se-091: a control from
# hours earlier measured residency, not the preset); its frames hold against that control's.
IFS=$'\t' read -r PN _ PM _ _ <<< "$(grep -m1 $'^Skater\t' "$HERE/scenarios.tsv")"
PRESET_REQ=(references:="[{\"name\": \"$PN\", \"kind\": \"character\", \"image\": \"$REF\"}]" prompt="<$PN> $PM" seed:=$((R + 101)) duration_s:=15)
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
  expect h3-preset-fast-time "$(awk -v f="$df" -v b="$db" 'BEGIN { print (b > 0 && f <= 0.90 * b) ? "PASS" : "FAIL" }')" "denoise fast ${df} ms vs balanced (run just before) ${db} ms"
  judge h3-preset-fast-frames h3-preset-fast '.contrast >= 0.5 * .control_contrast and .coherence >= 0.6' variant "$(video_of h3-preset-balanced)"
fi
