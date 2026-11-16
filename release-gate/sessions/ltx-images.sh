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
# Each cell keeps its run record, its video, a contact sheet ($OUT/sheets/<cell>.jpg: the first picture, frames 0, 30, 60, 90 and
# 120, the last picture) and, for each picture, its SSIM against every frame of the clip (scaled and centre-cropped to it, as
# Diffusers prepares it): the frame it matches best and the SSIM at frames 0 and 120. One rental; the pod's host RAM must hold the
# 67 GB model (`ram_gb`). params:
#   cells     what to run (default: all six)
#   seed      2611160            runtime   the Runtime manylinux wheel, https://…#sha256=<hex> (default: RUNTIME_WHEEL)
#   ram_gb    96                 need_s    seconds the cap must buy (5400)
. "$(dirname "$0")/lib.sh"
CELLS=$(param cells "was-text,text,first,last,first-last,was-ref") SEED=$(param seed 2611160) URL=$(param runtime)
RAM=$(param ram_gb 96) NEED=$(param need_s 5400)
WAS=$PACKAGES/was/ltx-2.5 NOW=$PACKAGES/now/ltx-2.5 FIX=$(cd "$(dirname "$0")" && pwd)/fixtures/ltx-images
for t in "$WAS" "$NOW"; do [ -f "$t/pyproject.toml" ] || fail "no packages_refs checkout ${t#"$PACKAGES"/}"; done
grep -q 'last_frame' "$NOW/ltx_2_5/__init__.py" || fail "now's ltx-2.5 takes no last_frame"
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
for p in lighthouse lantern teapot pearl; do
  run_page import "$OUT/$p.jpg" --name "$p" > "$OUT/import-$p.txt" 2>&1 || fail "import $p: $(cat "$OUT/import-$p.txt")"
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
# cell <name> <package dir> <fields json> [first picture] [last picture]: drop, render on the pod, keep and measure what it made.
cell() {
  local n=$1 pkg=$2 fields=$3 a=${4:-} b=${5:-} made run vid
  say "cell $n: $(jq -c . <<< "$fields")"
  made=$(run_page add "$n" --tool "$pkg/generate" --fields "$fields" 2>&1)
  J --arg n "$n" '.name == $n' <<< "$made" || { say "MISS $n: drop: $made"; bad+=("$n"); return 1; }
  run_page asset "$n" > "$OUT/runs/$n-asset-before.json" 2>&1
  run_page render --on "$NAME" "$n" > "$OUT/runs/$n-render.txt" 2>&1 || { say "MISS $n: $(tail -c 500 "$OUT/runs/$n-render.txt")"; bad+=("$n"); return 1; }
  run=$(newest_run "$n")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$run" --once) > "$OUT/runs/$n.json" 2>&1
  (cd "$RUN_PROJECT" && cozy --full run watch "$run" --once) > "$OUT/runs/$n.txt" 2>&1
  run_page asset "$n" > "$OUT/runs/$n-asset.json" 2>&1
  vid=$(file_of "$n" video) || { say "MISS $n: no video"; bad+=("$n"); return 1; }
  cp "$vid" "$OUT/videos/$n.mp4"
  say "$n: run $(jq -r '.run.number // .number // empty' "$OUT/runs/$n.json" 2>/dev/null | head -1) $(size "$vid"), $(frames "$vid") frames, $(ffprobe -v error -show_entries format=duration -of csv=p=0 "$vid") s"
  sheet "$n" "$vid" "$a" "$b"
  [ -z "$a" ] || say "$n first ($(basename "$a")): $(matched "$vid" "$a" "$n-first")"
  [ -z "$b" ] || say "$n last ($(basename "$b")): $(matched "$vid" "$b" "$n-last")"
}
has() { [[ ",$CELLS," == *",$1,"* ]]; }
bad=()
TEXT="A young woman with a blue headscarf walks slowly through a sunlit outdoor market, turns her head toward the camera and smiles. A medium shot, the camera tracks beside her. Market chatter and distant gulls."
base() { jq -cn --arg p "$1" --argjson s "$SEED" '{prompt: $p, resolution: "720p", duration_s: 5, seed: $s}'; }

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
has text && cell text "$NOW" "$(base "$TEXT")"
if has was-text && has text && [ -s "$OUT/videos/was-text.mp4" ] && [ -s "$OUT/videos/text.mp4" ]; then
  ffmpeg -v error -i "$OUT/videos/text.mp4" -i "$OUT/videos/was-text.mp4" -lavfi "[0:v][1:v]ssim=stats_file=$OUT/ssim-text-vs-was.txt" -f null - &&
    say "text vs was-text: $(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) { v = substr($i, 5) + 0; s += v; n++; if (!seen || v < low) { low = v; seen = 1 } } } END { printf "SSIM mean %.4f, lowest %.4f over %d frames", s / n, low, n }' "$OUT/ssim-text-vs-was.txt")"
  [ "$(sha256sum < "$OUT/videos/text.mp4")" = "$(sha256sum < "$OUT/videos/was-text.mp4")" ] && say "text and was-text are byte-identical"
fi
has first && cell first "$NOW" "$(base "The camera slowly pushes in on the lighthouse on its snowy sea cliff as waves crash against the rocks below and snow keeps falling. Howling wind and surf." | jq -c '. + {first_frame: "<lighthouse>"}')" "$OUT/lighthouse.jpg"
has last && cell last "$NOW" "$(base "A cozy café table by a rainy window; the camera drifts slowly toward a ceramic teapot beside a glowing lamp and settles on it. Rain patters on the glass." | jq -c '. + {last_frame: "<teapot>"}')" "" "$OUT/teapot.jpg"
has first-last && cell first-last "$NOW" "$(base "The camera rises and pushes in from the sea cliff to the lantern room at the top of the lighthouse while snow falls. Howling wind and surf." | jq -c '. + {first_frame: "<lighthouse>", last_frame: "<lantern>"}')" "$OUT/lighthouse.jpg" "$OUT/lantern.jpg"
if has was-ref; then
  cell was-ref "$WAS" "$(base "<pearl> walks slowly through a sunlit outdoor market, turns her head toward the camera and smiles. A medium shot, the camera tracks beside her. Market chatter and distant gulls.")" "$OUT/pearl.jpg"
  say "was-ref unused: $(jq -c '.asset.unused // []' "$OUT/runs/was-ref-asset.json")"
  say "was-ref notes: $(jq -c '[.. | objects | select(.code? == "mention_unused") | .message] | unique' "$OUT/runs/was-ref.json" 2>/dev/null)"
  J '.asset.unused | any(.mention == "pearl")' "$OUT/runs/was-ref-asset.json" || { say "MISS was-ref: the box doesn't say <pearl> is unused"; bad+=(was-ref-unused); }
fi
[ "${#bad[@]}" = 0 ] || fail "missed: ${bad[*]}"
say "PASS: every cell rendered; judge $OUT/sheets (each picture beside the frames it should be) and the SSIM lines above"
