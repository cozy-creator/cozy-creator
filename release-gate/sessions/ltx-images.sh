# ltx-images: LTX-2.5's pictures as the page renders them (e2e/page.py), on one rental. Paul, 11-16: "ltx 2.5 does not work with
# image references", "I just had a reference to an image and it did not do anything with the character". packages_refs: `was` (the
# RC's ltx-2.5, the release's behaviour) and `now` (the change). Every cell renders 720p, 5 s (121 frames), one seed, with sound.
# Cells (`cells`, comma-separated, in this order):
#   was-text    was: text only
#   text        now: the same text; held against was-text frame by frame (one conditioning pipeline now serves text too)
#   first       now: first_frame=<lighthouse>
#   last        now: last_frame=<teapot>
#   first-last  now: first_frame=<lighthouse>, last_frame=<lantern> (the lighthouse's lantern room, cropped from it)
#   was-ref     was: <pearl> mentioned in the prompt: the box says the mention is unused, the run carries a mention_unused note
#   ref         now: <pearl> mentioned: the Ingredients reference sheet; the box keeps the mention (no `unused`)
#   ref-first   now: <pearl> mentioned and first_frame=<teapot> (a combination Ingredients was not trained on)
#   sound       now: <beat> mentioned, a synthetic two-beats-a-second drum: the soundtrack, returned as given
#   pickup      now: text's take picked up at frame 72: the take's first 72 frames kept, 96 new (5 s less the 25 carried)
#   text-up, first-up, ref-up   text, first and ref with prompt_upsampling on (Lightricks' Gemma 4 E2B enhancer)
#   video       now: <clip> (was-text's take, else text's) mentioned: the Union Control reference, by its depth (the
#               default); the output's edges held against the clip's (ffmpeg edgedetect, SSIM) beside text's, which had none
#   video-edges the same by the clip's edges
#   motion      motion transfer (Paul 11-16): first_frame=<pearl>, "moves the way <clip> does", control=pose
#   keys        now: first_frame=<lighthouse>, a keyframe <lantern> at 2.5 s (frame 60): it lands on frame 60
#   t2a         now's `sound` (Lightricks' audio-only text-to-audio): 5 s of a line of speech and a door; its FLAC, its
#               length and level, and a spectrogram ($OUT/sheets/t2a.png)
#   retake      now's `retake` of text's take, 2-3.5 s redone (picture and sound): each frame's SSIM to the take, outside
#               the latents redone (frames 0-36 and 96-120) and inside (48-84), and the sound's difference in each
#   retake-sound the same with redo=sound: the picture held against the take, the sound redone in the window
#   voice       now: <pearl> speaks in <line> (t2a's sound): Licon MSR's picture and voice; the model reads
#               `Image 1: pearl.` and `Audio 1: line, belongs only to Image 1.`, and the box keeps both mentions
# The `now` cells render with prompt_upsampling off unless named `-up`; each keeps the text the model read
# ($OUT/sheets/<cell>.txt, the result's `prompt`) beside its sheet.
# Each cell keeps its run record, its video, a contact sheet ($OUT/sheets/<cell>.jpg: the first picture, frames 0, 30, 60, 90 and
# 120, the last picture) and, for each picture, its SSIM against every frame of the clip (scaled and centre-cropped to it, as
# Diffusers prepares it): the frame it matches best and the SSIM at frames 0 and 120. One rental; the pod's host RAM must hold the
# 67 GB model (`ram_gb`). params:
#   cells     what to run (default: the first six)
#   seed      2611160            runtime   the Runtime manylinux wheel, https://…#sha256=<hex> (default: RUNTIME_WHEEL)
#   ram_gb    96                 need_s    seconds the cap must buy (5400)
. "$(dirname "$0")/lib.sh"
CELLS=$(param cells "was-text,text,first,last,first-last,was-ref") SEED=$(param seed 2611160) URL=$(param runtime)
RAM=$(param ram_gb 96) NEED=$(param need_s 5400)
WAS=$PACKAGES/was/ltx-2.5 NOW=$PACKAGES/now/ltx-2.5 FIX=$(cd "$(dirname "$0")" && pwd)/fixtures/ltx-images
for t in "$WAS" "$NOW"; do [ -f "$t/pyproject.toml" ] || fail "no packages_refs checkout ${t#"$PACKAGES"/}"; done
grep -q 'last_frame' "$NOW/ltx_2_5/__init__.py" || fail "now's ltx-2.5 takes no last_frame"
for c in ref ref-up ref-first sound pickup text-up first-up video video-edges motion keys; do
  [[ ",$CELLS," != *",$c,"* ]] || grep -q 'prompt: Prompt\[Image' "$NOW/ltx_2_5/__init__.py" || fail "now's ltx-2.5 is older than references (cell $c)"
done
for c in t2a:sound retake:retake retake-sound:retake voice:generate; do
  [[ ",$CELLS," != *",${c%%:*},"* ]] || grep -q "^def ${c#*:}(" "$NOW/ltx_2_5/__init__.py" || fail "now's ltx-2.5 has no ${c#*:} tool (cell ${c%%:*})"
done
[[ ",$CELLS," != *",voice,"* ]] || { grep -q 'speakers(' "$NOW/ltx_2_5/__init__.py" && [[ ",$CELLS," == *",t2a,"* ]]; } ||
  fail "cell voice needs now's ltx-2.5 with MSR voices, and cell t2a for its line"
[ -f "$FIX/pearl.jpg" ] && [ -f "$FIX/teapot.jpg" ] && [ -f "$REF/seed.jpg" ] || fail "no fixtures"
command -v ffprobe > /dev/null || fail "no ffprobe (pod-session.yaml installs it from the tool cache)"
mkdir -p "$OUT/wheel" "$OUT/sheets" "$OUT/videos" "$OUT/runs"
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}" | sed 's/%2B/+/g')
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
else
  W=${RUNTIME_WHEEL:-}
  [ -n "$W" ] || fail "no Runtime: name \`runtime\`, or dispatch from rc/<x>"
fi
J() { jq -e "$@" > /dev/null; }
# The pictures: the gate's lighthouse, its lantern room cropped from it, a teapot (the bench's) and Vermeer's Girl with a Pearl
# Earring (public domain, Wikimedia Commons' 1280 px rendition, scaled to 768).
cp "$REF/seed.jpg" "$OUT/lighthouse.jpg"; cp "$FIX/pearl.jpg" "$OUT/pearl.jpg"
ffmpeg -v error -y -i "$REF/seed.jpg" -vf "crop=420:420:178:40" "$OUT/lantern.jpg" || fail "lantern crop"
cp "$FIX/teapot.jpg" "$OUT/teapot.jpg"
# Two drum beats a second for 6 s: a 60 Hz thump and a click, each decaying in 0.12 s, stereo 48 kHz.
ffmpeg -v error -y -f lavfi -i "aevalsrc='0.9*sin(2*PI*60*t)*exp(-25*mod(t,0.5))+0.3*sin(2*PI*2400*t)*exp(-90*mod(t,0.5))':s=48000:d=6:c=stereo" \
  -c:a pcm_s16le "$OUT/beat.wav" || fail "beat"
for p in lighthouse lantern teapot pearl beat; do
  f=$OUT/$p.jpg; [ -f "$f" ] || f=$OUT/$p.wav
  run_page import "$f" --name "$p" > "$OUT/import-$p.txt" 2>&1 || fail "import $p: $(cat "$OUT/import-$p.txt")"
done

newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r --arg f "$2" '.versions[-1].files[$f].path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 600 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$1"; }
frames() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$1"; }
# matched <video> <picture> <label>: the picture's SSIM against each frame, scaled and centre-cropped to the video.
matched() {
  local s n
  s=$(size "$1"); n=$(frames "$1")
  ffmpeg -v error -i "$1" -framerate 24 -loop 1 -i "$2" -lavfi "[1:v]scale=${s%x*}:${s#*x}:force_original_aspect_ratio=increase,crop=${s%x*}:${s#*x},format=yuv420p,trim=end_frame=$n[p];[0:v]format=yuv420p[v];[v][p]ssim=stats_file=$OUT/ssim-$3.txt" -frames:v "$n" -f null - \
    || { say "$3: no SSIM"; return 1; }
  awk -v n="$n" '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { v = substr($i, 5) + 0; f = NR - 1; s[f] = v; if (!seen || v > best) { best = v; at = f; seen = 1 } } }
    END { printf "best frame %d of %d (SSIM %.3f); frame 0 %.3f, frame %d %.3f", at, n - 1, best, s[0], n - 1, s[n - 1] }' "$OUT/ssim-$3.txt"
}
# sheet <cell> <video> [first picture] [last picture]: the pictures either side of frames 0, 30, 60, 90 and 120.
sheet() {
  local inputs=(-i "$2") graph="[0:v]select='eq(n,0)+eq(n,30)+eq(n,60)+eq(n,90)+eq(n,120)',scale=-2:256,tile=5x1:padding=4[f];" row='' k=0 i=1
  if [ -n "${3:-}" ]; then inputs+=(-i "$3"); graph+="[$i:v]scale=-2:256[a];"; row+='[a]'; k=$((k + 1)); i=$((i + 1)); fi
  row+='[f]'; k=$((k + 1))
  if [ -n "${4:-}" ]; then inputs+=(-i "$4"); graph+="[$i:v]scale=-2:256[b];"; row+='[b]'; k=$((k + 1)); fi
  if [ "$k" = 1 ]; then graph+='[f]null[o]'; else graph+="${row}hstack=inputs=$k[o]"; fi
  ffmpeg -v error -y "${inputs[@]}" -filter_complex "$graph" -map "[o]" -frames:v 1 "$OUT/sheets/$1.jpg" || say "$1: no sheet"
}
# render <name> <tool> <fields json>: drop the box, render it on the pod and keep its run record.
render() {
  local n=$1 made run
  say "cell $n: $(jq -c . <<< "$3")"
  made=$(run_page add "$n" --tool "$2" --fields "$3" 2>&1)
  J --arg n "$n" '.name == $n' <<< "$made" || { say "MISS $n: drop: $made"; bad+=("$n"); return 1; }
  run_page asset "$n" > "$OUT/runs/$n-asset-before.json" 2>&1
  run_page render --on "$NAME" "$n" > "$OUT/runs/$n-render.txt" 2>&1 || { say "MISS $n: $(tail -c 500 "$OUT/runs/$n-render.txt")"; bad+=("$n"); return 1; }
  run=$(newest_run "$n")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$run" --once) > "$OUT/runs/$n.json" 2>&1
  (cd "$RUN_PROJECT" && cozy --full run watch "$run" --once) > "$OUT/runs/$n.txt" 2>&1
  run_page asset "$n" > "$OUT/runs/$n-asset.json" 2>&1
}
# cell <name> <package dir, or dir/tool> <fields json> [first picture] [last picture]: render (`generate` unless named),
# keep and measure the video it made.
cell() {
  local n=$1 tool=$2 fields=$3 a=${4:-} b=${5:-} vid
  [ -f "$tool/pyproject.toml" ] && tool=$tool/generate
  render "$n" "$tool" "$fields" || return 1
  vid=$(file_of "$n" video) || { say "MISS $n: no video"; bad+=("$n"); return 1; }
  cp "$vid" "$OUT/videos/$n.mp4"
  jq -r '[.. | objects | select(has("frame_count") and has("prompt")) | .prompt] | first // empty' "$OUT/runs/$n.json" > "$OUT/sheets/$n.txt"
  [ -s "$OUT/sheets/$n.txt" ] && say "$n read: $(head -c 1500 "$OUT/sheets/$n.txt" | tr '\n' ' ')"
  say "$n: run $(jq -r '.run.number // .number // empty' "$OUT/runs/$n.json" 2>/dev/null | head -1) $(size "$vid"), $(frames "$vid") frames, $(ffprobe -v error -show_entries format=duration -of csv=p=0 "$vid") s"
  sheet "$n" "$vid" "$a" "$b"
  [ -z "$a" ] || say "$n first ($(basename "$a")): $(matched "$vid" "$a" "$n-first")"
  [ -z "$b" ] || say "$n last ($(basename "$b")): $(matched "$vid" "$b" "$n-last")"
}
has() { [[ ",$CELLS," == *",$1,"* ]]; }
bad=()
TEXT="A young woman with a blue headscarf walks slowly through a sunlit outdoor market, turns her head toward the camera and smiles. A medium shot, the camera tracks beside her. Market chatter and distant gulls."
base() { jq -cn --arg p "$1" --argjson s "$SEED" '{prompt: $p, resolution: "720p", duration_s: 5, seed: $s}'; }
# now <prompt> [up]: a now cell's fields, its prompt_upsampling off unless `up`.
now() { base "$1" | jq -c --argjson u "$([ "${2:-}" = up ] && echo true || echo false)" '. + {prompt_upsampling: $u}'; }

quote "$NEED"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
rent
mem=$(timeout 120 cozy machine ssh "$NAME" "awk '/MemTotal/{print int(\$2/1048576)}' /proc/meminfo" 2>/dev/null | grep -E '^[0-9]+$' | tail -1)
say "pod host RAM: ${mem:-unknown} GiB"
[ -n "$mem" ] && [ "$mem" -ge "$RAM" ] || fail "host RAM ${mem:-unknown} GiB is under $RAM: dispatch again with sku=a40"
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

has was-text && cell was-text "$WAS" "$(base "$TEXT")"
has text && cell text "$NOW" "$(now "$TEXT")"
has text-up && cell text-up "$NOW" "$(now "$TEXT" up)"
if has was-text && has text && [ -s "$OUT/videos/was-text.mp4" ] && [ -s "$OUT/videos/text.mp4" ]; then
  ffmpeg -v error -i "$OUT/videos/text.mp4" -i "$OUT/videos/was-text.mp4" -lavfi "[0:v][1:v]ssim=stats_file=$OUT/ssim-text-vs-was.txt" -f null - &&
    say "text vs was-text: $(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { v = substr($i, 5) + 0; s += v; n++; if (!seen || v < low) { low = v; seen = 1 } } } END { printf "SSIM mean %.4f, lowest %.4f over %d frames", s / n, low, n }' "$OUT/ssim-text-vs-was.txt")"
  [ "$(sha256sum < "$OUT/videos/text.mp4")" = "$(sha256sum < "$OUT/videos/was-text.mp4")" ] && say "text and was-text are byte-identical"
  ffmpeg -v error -y -i "$OUT/sheets/was-text.jpg" -i "$OUT/sheets/text.jpg" -filter_complex vstack "$OUT/sheets/text-vs-was.jpg" || true
fi
FIRST="The camera slowly pushes in on the lighthouse on its snowy sea cliff as waves crash against the rocks below and snow keeps falling. Howling wind and surf."
has first && cell first "$NOW" "$(now "$FIRST" | jq -c '. + {first_frame: "<lighthouse>"}')" "$OUT/lighthouse.jpg"
has first-up && cell first-up "$NOW" "$(now "$FIRST" up | jq -c '. + {first_frame: "<lighthouse>"}')" "$OUT/lighthouse.jpg"
has last && cell last "$NOW" "$(now "A cozy café table by a rainy window; the camera drifts slowly toward a ceramic teapot beside a glowing lamp and settles on it. Rain patters on the glass." | jq -c '. + {last_frame: "<teapot>"}')" "" "$OUT/teapot.jpg"
has first-last && cell first-last "$NOW" "$(now "The camera rises and pushes in from the sea cliff to the lantern room at the top of the lighthouse while snow falls. Howling wind and surf." | jq -c '. + {first_frame: "<lighthouse>", last_frame: "<lantern>"}')" "$OUT/lighthouse.jpg" "$OUT/lantern.jpg"
if has was-ref; then
  cell was-ref "$WAS" "$(base "<pearl> walks slowly through a sunlit outdoor market, turns her head toward the camera and smiles. A medium shot, the camera tracks beside her. Market chatter and distant gulls.")" "$OUT/pearl.jpg"
  say "was-ref unused: $(jq -c '.asset.unused // []' "$OUT/runs/was-ref-asset.json")"
  say "was-ref notes: $(jq -c '[.. | objects | select(.code? == "mention_unused") | .message] | unique' "$OUT/runs/was-ref.json" 2>/dev/null)"
  J '.asset.unused | any(.mention == "pearl")' "$OUT/runs/was-ref-asset.json" || { say "MISS was-ref: the box doesn't say <pearl> is unused"; bad+=(was-ref-unused); }
fi
REFP="<pearl> walks slowly through a sunlit outdoor market, turns her head toward the camera and smiles. A medium shot, the camera tracks beside her. Market chatter and distant gulls."
for c in ref ref-up; do
  has "$c" || continue
  cell "$c" "$NOW" "$(now "$REFP" "${c#ref-}")" "$OUT/pearl.jpg"
  J '(.asset.unused // []) | any(.mention == "pearl") | not' "$OUT/runs/$c-asset.json" || { say "MISS $c: the box says <pearl> is unused"; bad+=("$c-unused"); }
  grep -q '^Reference sheet: Center: pearl\.' "$OUT/sheets/$c.txt" || { say "MISS $c: the model read no reference sheet"; bad+=("$c-sheet"); }
done
has ref-first && cell ref-first "$NOW" "$(now "<pearl> sits down at the café table by the rainy window and pours tea from the teapot, then looks up at the camera. Rain patters on the glass." | jq -c '. + {first_frame: "<teapot>"}')" "$OUT/teapot.jpg"
if has sound; then
  cell sound "$NOW" "$(now "<beat> A drummer in a dim rehearsal room strikes a large bass drum, one hit on every beat, the drum skin shaking with each hit. A medium shot, the camera remains static.")"
  if [ -s "$OUT/videos/sound.mp4" ]; then
    ffmpeg -v error -i "$OUT/videos/sound.mp4" -i "$OUT/beat.wav" -filter_complex "[0:a]aresample=48000,atrim=end=5[a];[1:a]atrim=end=5[b];[a][b]amix=inputs=2:weights='1 -1',astats=metadata=1:reset=0,ametadata=print:key=lavfi.astats.Overall.RMS_level:file=$OUT/sound-diff.txt" -f null - 2>/dev/null
    say "sound: the clip's audio less the beat, RMS $(tail -1 "$OUT/sound-diff.txt" | cut -d= -f2) dB (the beat itself: about -10 dB)"
  fi
fi
if has pickup && has text && [ -s "$OUT/videos/text.mp4" ]; then
  say "pickup: text's take from frame 72"
  run_page render --on "$NAME" --pickup '<text.1>' 72 text > "$OUT/runs/pickup-render.txt" 2>&1 || { say "MISS pickup: $(tail -c 500 "$OUT/runs/pickup-render.txt")"; bad+=(pickup); }
  run=$(newest_run text)
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$run" --once) > "$OUT/runs/pickup.json" 2>&1
  vid=$(file_of text video) && cp "$vid" "$OUT/videos/pickup.mp4" && {
    say "pickup: $(size "$vid"), $(frames "$vid") frames (72 kept + 96 new = 168)"
    ffmpeg -v error -i "$OUT/videos/pickup.mp4" -i "$OUT/videos/text.mp4" -lavfi "[0:v]trim=end_frame=72,setpts=N/FRAME_RATE/TB[a];[1:v]trim=end_frame=72,setpts=N/FRAME_RATE/TB[b];[a][b]ssim=stats_file=$OUT/ssim-pickup-kept.txt" -f null - &&
      say "pickup kept: $(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { v = substr($i, 5) + 0; s += v; n++; if (!seen || v < low) { low = v; seen = 1 } } } END { printf "SSIM to the take mean %.4f, lowest %.4f over %d frames", s / n, low, n }' "$OUT/ssim-pickup-kept.txt")"
    ffmpeg -v error -y -i "$OUT/videos/pickup.mp4" -vf "select='between(n,66,78)',scale=-2:256,tile=13x1:padding=4" -frames:v 1 "$OUT/sheets/pickup-join.jpg"
    sheet pickup "$OUT/videos/pickup.mp4"
  }
fi
# edged <video> <frames>: its edges (ffmpeg's Canny), first <frames> frames, for holding structure against structure.
edged() { ffmpeg -v error -y -i "$1" -vf "scale=640:-2,edgedetect=low=0.1:high=0.3,trim=end_frame=$2" -an "$OUT/edges-$(basename "$1")"; }
edge_ssim() {
  ffmpeg -v error -i "$OUT/edges-$(basename "$1")" -i "$OUT/edges-$(basename "$2")" -lavfi "[0:v]setpts=N/FRAME_RATE/TB[a];[1:v]setpts=N/FRAME_RATE/TB,scale=640:-2[b];[a][b]ssim=stats_file=$OUT/ssim-edges.txt" -f null - &&
    awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { s += substr($i, 5); n++ } } END { printf "%.4f", s / n }' "$OUT/ssim-edges.txt"
}
if has video || has video-edges || has motion; then
  src=$OUT/videos/was-text.mp4; [ -s "$src" ] || src=$OUT/videos/text.mp4
  if [ -s "$src" ]; then
    cp "$src" "$OUT/clip.mp4"
    run_page import "$OUT/clip.mp4" --name clip > "$OUT/import-clip.txt" 2>&1 || fail "import clip: $(cat "$OUT/import-clip.txt")"
    edged "$OUT/clip.mp4" 121; [ -s "$OUT/videos/text.mp4" ] && edged "$OUT/videos/text.mp4" 121
    FOX="A watercolor painting comes to life: a fox in a red scarf walks through an autumn forest the way <clip> moves, turns its head toward the camera. Rustling leaves."
    for c in video video-edges motion; do
      has "$c" || continue
      case $c in
        video) cell "$c" "$NOW" "$(now "$FOX")" ;;
        video-edges) cell "$c" "$NOW" "$(now "$FOX" | jq -c '. + {control: "edges"}')" ;;
        motion) cell "$c" "$NOW" "$(now "The girl in the painting walks slowly through a sunlit outdoor market and turns her head toward the camera, moving the way <clip> does. Market chatter." | jq -c '. + {control: "pose", first_frame: "<pearl>"}')" "$OUT/pearl.jpg" ;;
      esac
      [ -s "$OUT/videos/$c.mp4" ] || continue
      edged "$OUT/videos/$c.mp4" 121
      say "$c: edges against the clip's, SSIM $(edge_ssim "$OUT/videos/$c.mp4" "$OUT/clip.mp4"); text's (no reference) $([ -s "$OUT/videos/text.mp4" ] && edge_ssim "$OUT/videos/text.mp4" "$OUT/clip.mp4")"
      ffmpeg -v error -y -i "$OUT/clip.mp4" -i "$OUT/videos/$c.mp4" -filter_complex "[0:v]select='eq(n,0)+eq(n,60)+eq(n,120)',scale=-2:256,tile=3x1[a];[1:v]select='eq(n,0)+eq(n,60)+eq(n,120)',scale=-2:256,tile=3x1[b];[a][b]vstack" -frames:v 1 "$OUT/sheets/$c-vs-clip.jpg" || true
    done
  else
    say "MISS video: no take to reference"; bad+=(video)
  fi
fi
if has keys; then
  cell keys "$NOW" "$(now "The camera rises from the snowy sea cliff to the lantern room at the top of the lighthouse and holds there while snow falls. Howling wind." | jq -c '. + {first_frame: "<lighthouse>", keyframes: [{picture: "<lantern>", at_s: 2.5}]}')" "$OUT/lighthouse.jpg"
  [ -s "$OUT/videos/keys.mp4" ] && say "keys: the lantern keyframe (frame 60): $(matched "$OUT/videos/keys.mp4" "$OUT/lantern.jpg" keys-key)"
fi
# ssim_range <a> <b> <from> <to> <label>: mean SSIM of frames from..to (inclusive) of two videos of one size.
ssim_range() {
  ffmpeg -v error -i "$1" -i "$2" -lavfi "[0:v]trim=start_frame=$3:end_frame=$(($4 + 1)),setpts=N/FRAME_RATE/TB[a];[1:v]trim=start_frame=$3:end_frame=$(($4 + 1)),setpts=N/FRAME_RATE/TB[b];[a][b]ssim=stats_file=$OUT/ssim-$5.txt" -f null - &&
    awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { s += substr($i, 5); n++ } } END { printf "%.4f", s / n }' "$OUT/ssim-$5.txt"
}
# rms_diff <a> <b> <from s> <to s>: the RMS level (dB) of a's sound less b's, between two times; -inf is identical.
rms_diff() {
  ffmpeg -v error -i "$1" -i "$2" -filter_complex "[0:a]aresample=48000,atrim=$3:$4[a];[1:a]aresample=48000,atrim=$3:$4[b];[a][b]amix=inputs=2:normalize=0:weights='1 -1',astats=metadata=1:reset=0,ametadata=print:key=lavfi.astats.Overall.RMS_level:file=$OUT/rms.txt" -f null - 2>/dev/null &&
    tail -1 "$OUT/rms.txt" | cut -d= -f2
}
if has t2a; then
  render t2a "$NOW/sound" "$(jq -cn --argjson s "$SEED" '{prompt: "A woman saying: \"Oh, what a lovely day we are having!\" Then a wooden door creaks open and shuts.", duration_s: 5, seed: $s}')" &&
    if f=$(file_of t2a audio); then
      cp "$f" "$OUT/videos/t2a.flac"
      say "t2a: $(ffprobe -v error -show_entries stream=codec_name,sample_rate,channels:format=duration -of compact=p=0 "$f" | tr '\n' ' ')"
      say "t2a level: $(ffmpeg -v error -i "$f" -af "astats=metadata=1:reset=0,ametadata=print:key=lavfi.astats.Overall.RMS_level:file=$OUT/t2a-rms.txt" -f null - && tail -1 "$OUT/t2a-rms.txt" | cut -d= -f2) dB RMS"
      jq -r '[.. | objects | select(has("sample_rate") and has("prompt")) | .prompt] | first // empty' "$OUT/runs/t2a.json" > "$OUT/sheets/t2a.txt"
      ffmpeg -v error -y -i "$f" -lavfi "showspectrumpic=s=1024x384:legend=1" "$OUT/sheets/t2a.png" || say "t2a: no spectrogram"
    else say "MISS t2a: no audio"; bad+=(t2a); fi
fi
if has voice; then
  if [ -s "$OUT/videos/t2a.flac" ]; then
    run_page import "$OUT/videos/t2a.flac" --name line > "$OUT/import-line.txt" 2>&1 || fail "import line: $(cat "$OUT/import-line.txt")"
    cell voice "$NOW" "$(now '<pearl> sits by a sunlit window, turns to the camera and says, in <line>: "Oh, what a lovely day we are having!" A medium close-up, the camera remains static.')" "$OUT/pearl.jpg"
    if [ -s "$OUT/sheets/voice.txt" ]; then
      grep -q '^Image 1: pearl\.' "$OUT/sheets/voice.txt" && grep -q '^Audio 1: line, belongs only to Image 1\.' "$OUT/sheets/voice.txt" ||
        { say "MISS voice: the model read no MSR cast"; bad+=(voice-cast); }
      J '(.asset.unused // []) | length == 0' "$OUT/runs/voice-asset.json" || { say "MISS voice: the box says a mention is unused"; bad+=(voice-unused); }
    fi
  else
    say "MISS voice: no t2a line to speak in"; bad+=(voice)
  fi
fi
if has retake || has retake-sound; then
  take=$OUT/videos/text.mp4; [ -s "$take" ] || take=$OUT/videos/was-text.mp4
  if [ -s "$take" ]; then
    cp "$take" "$OUT/take.mp4"
    run_page import "$OUT/take.mp4" --name take > "$OUT/import-take.txt" 2>&1 || fail "import take: $(cat "$OUT/import-take.txt")"
    AGAIN="The young woman with the blue headscarf stops, laughs and claps her hands twice, then walks on. A medium shot, the camera tracks beside her. Market chatter, her laugh and two sharp claps."
    for c in retake retake-sound; do
      has "$c" || continue
      redo=both; [ "$c" = retake-sound ] && redo=sound
      cell "$c" "$NOW/retake" "$(jq -cn --arg p "$AGAIN" --arg r "$redo" --argjson s "$SEED" '{video: "<take>", start_s: 2, end_s: 3.5, prompt: $p, redo: $r, seed: $s}')" || continue
      v=$OUT/videos/$c.mp4
      # 2-3.5 s meets latent frames 6-11 (pixel frames 41-88); the rest are the take's latents, decoded again.
      say "$c: SSIM to the take, frames 0-36 $(ssim_range "$v" "$OUT/take.mp4" 0 36 "$c-a"), 48-84 $(ssim_range "$v" "$OUT/take.mp4" 48 84 "$c-in"), 96-120 $(ssim_range "$v" "$OUT/take.mp4" 96 120 "$c-b")"
      say "$c: sound less the take's, 0-1.6 s $(rms_diff "$v" "$OUT/take.mp4" 0 1.6) dB, 2-3.5 s $(rms_diff "$v" "$OUT/take.mp4" 2 3.5) dB, 4-5 s $(rms_diff "$v" "$OUT/take.mp4" 4 5) dB"
      ffmpeg -v error -y -i "$OUT/take.mp4" -i "$v" -filter_complex "[0:v]select='eq(n,24)+eq(n,60)+eq(n,72)+eq(n,108)',scale=-2:256,tile=4x1[a];[1:v]select='eq(n,24)+eq(n,60)+eq(n,72)+eq(n,108)',scale=-2:256,tile=4x1[b];[a][b]vstack" -frames:v 1 "$OUT/sheets/$c-vs-take.jpg" || true
    done
  else
    say "MISS retake: no take to redo"; bad+=(retake)
  fi
fi
[ "${#bad[@]}" = 0 ] || fail "missed: ${bad[*]}"
say "PASS: every cell rendered; judge $OUT/sheets (each picture beside the frames it should be) and the SSIM lines above"
