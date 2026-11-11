# tool-functions (cl-774, "A tool is a function"): each tool from dropping it to accepting its output, on one rental driven from a
# runner, the way the page does it (e2e/page.py): POST assets {tool, fields}, render, approve. What e2e's weightless `shaped.py` can't
# show goes here: frames, a plate, a take, several pictures and a silent shot on the real models, and a file as the upgrade writes it,
# text asset and all. Packages are local code from a packages_refs checkout named `pk` (minimax-h3 0.10 and on); the session's cozy
# carries its TensorD to the pod, and `runtime` the Runtime. A completed run is no pass: each output is measured, its frames kept for
# a person to judge in $OUT/frames, and each call's input is checked against what crossed its mentions. One rental (a 4090). params:
#   runtime     the Runtime wheel, https://…#sha256=<hex> (default: the session's RUNTIME_WHEEL, from rc/<x>)
. "$(dirname "$0")/lib.sh"
URL=$(param runtime) PK=$PACKAGES/pk
for p in qwen-image-2 minimax-h3 birefnet seedvr2; do [ -f "$PK/$p/pyproject.toml" ] || fail "no packages_refs checkout pk/$p"; done
grep -q 'named="shot"' "$PK/minimax-h3/h3.py" || fail "pk is older than minimax-h3 0.10: its generate is no shot tool"
mkdir -p "$OUT/wheel" "$OUT/frames"
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}")
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
else
  W=${RUNTIME_WHEEL:-}
  [ -n "$W" ] || fail "no Runtime: name \`runtime\`, or dispatch from rc/<x>"
fi
command -v ffprobe > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
[ -f "$REF/seed.jpg" ] || fail "no reference image"
J() { jq -e "$@" > /dev/null; }
cp "$REF/seed.jpg" "$OUT/art.jpg"
run_page import "$OUT/art.jpg" --name art > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
J '.name == "art"' "$OUT/import.txt" || fail "the picture is no box art: $(cat "$OUT/import.txt")"
# drop <name> <tool> <fields json> <kind>: what dropping a tool writes, with its kind from its output.
drop() {
  local made
  made=$(run_page add "$1" --tool "$2" --fields "$3" 2>&1)
  J --arg n "$1" '.name == $n' <<< "$made" || fail "drop $1: $made"
  J --arg k "$4" '.asset.kind == $k' <<< "$(run_page asset "$1")" || fail "$1's kind isn't $4, what $2 outputs"
}
# render <name> [page.py render flags…]: rendered on the pod, followed until its new version lands.
render() {
  local n=$1; shift
  say "render $n on $NAME $*"
  run_page render --on "$NAME" "$@" "$n" > "$OUT/render-$n.txt" 2>&1
}
# accept <name>: render it on the pod, then approve its newest version, as a double tap on it does.
accept() {
  render "$1" || fail "$1: $(tail -c 500 "$OUT/render-$1.txt")"
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].number')
  run_page approve "$1" "$n" > "$OUT/approve-$1.txt" 2>&1 || fail "approve $1: $(tail -c 300 "$OUT/approve-$1.txt")"
  J '.asset.pinned' <<< "$(run_page asset "$1")" || fail "$1 isn't accepted"
}
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
call() { cozy --json --full run watch "$(newest_run "$1")" --once | jq -c .request; }
# log <name>: its newest run's record and log.
log() { cozy --full run watch "$(newest_run "$1")" --once; }
# file_of <name> <output>: its newest version's file of that output, once here (a version is listed ahead of a file on its way).
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r --arg f "$2" '.versions[-1].files[$f].path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 600 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$1"; }
seconds() { ffprobe -v error -show_entries format=duration -of csv=p=0 "$1"; }
keep_frames() { ffmpeg -v error -y -i "$2" -vf "select='eq(n,0)+eq(n,60)+eq(n,120)'" -vsync 0 "$OUT/frames/$1-%d.png"; }
# held <old> <new> <join frame>: how far `new` holds `old`'s frames up to the join, and whether the join shows. Each frame's PSNR, frame n of one
# against frame n of the other, and the step from each frame to the next in `new` beside its neighbours' (run 20751's join was a luma step of 3.7).
held() {
  local old=$1 new=$2 join=$3 log=$OUT/take-psnr.txt steps=$OUT/take-steps.txt luma=$OUT/take-luma.txt
  ffmpeg -v error -i "$new" -i "$old" -lavfi "[0:v]setpts=N/FRAME_RATE/TB[a];[1:v]setpts=N/FRAME_RATE/TB[b];[a][b]psnr=stats_file=$log" -frames:v "$join" -f null - \
    || fail "PSNR against the kept take"
  ffmpeg -v error -i "$new" -lavfi "split[a][b];[b]trim=start_frame=1,setpts=N/FRAME_RATE/TB[c];[a]setpts=N/FRAME_RATE/TB[d];[d][c]psnr=stats_file=$steps" -f null - \
    || fail "PSNR of each frame against the next"
  ffmpeg -v error -i "$new" -vf "signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=$luma" -f null - || fail "luma of each frame"
  say "held: $(awk -v join="$join" 'NR <= join { for (i = 1; i <= NF; i++) if ($i ~ /^psnr_avg:/) { v = substr($i, 10); v = (v == "inf") ? 100 : v + 0; n++; sum += v
      if (!seen || v < low) { low = v; at = n; seen = 1 } if (n > join - 24 && (!last || v < end)) { end = v; last = 1 } } }
    END { printf "%d frames up to the join at %d: PSNR mean %.2f dB, lowest %.2f dB at frame %d, lowest of the last 24 %.2f dB", n, join, sum / n, low, at, end }' "$log")"
  say "join: $(awk -v join="$join" '{ for (i = 1; i <= NF; i++) if ($i ~ /^psnr_avg:/) { p = substr($i, 10); s[NR] = (p == "inf") ? 100 : p + 0 } }
    END { for (i = join - 24; i <= join + 24; i++) if (i != join && i in s) { n++; v[n] = s[i] }
      for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (v[j] < v[i]) { t = v[i]; v[i] = v[j]; v[j] = t }
      printf "frame to next %.2f dB across it, %.2f dB is the median of 24 frames either side", s[join], v[int((n + 1) / 2)] }' "$steps"), \
$(awk -v join="$join" -F= '/YAVG/ { n++; y[n] = $2 } END { for (i = join - 23; i <= join + 25; i++) if (i != join + 1 && (i in y) && ((i - 1) in y)) { d = y[i] - y[i - 1]; m++; v[m] = (d < 0 ? -d : d) }
      for (i = 1; i <= m; i++) for (j = i + 1; j <= m; j++) if (v[j] < v[i]) { t = v[i]; v[i] = v[j]; v[j] = t }
      d = y[join + 1] - y[join]; printf "luma step %.2f across it, median %.2f", (d < 0 ? -d : d), v[int((m + 1) / 2)] }' "$luma")"
  ffmpeg -v error -y -i "$new" -vf "select='between(n,$((join - 1)),$join)'" -vsync 0 "$OUT/frames/join-%d.png"
  ffmpeg -v error -y -i "$old" -vf "select='eq(n,$((join - 1)))'" -vsync 0 "$OUT/frames/join-kept.png"
}

quote 3600
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

say "qwen-image-2/image: an image edit reading a picture, its description crossing and its instructions not"
drop clean "$PK/qwen-image-2/image" '{"instructions": "OWN-QWEN-PROMPT Keep the subject of <art> exactly as it is, on a plain light grey studio background.", "description": "PUBLIC-WORDS the subject on a plain grey background.", "aspect_ratio": "1:1", "megapixels": 1}' image
accept clean
J '(.target | test("/image(@|$)")) and (.input.instructions.references[0].key == "art") and .input.megapixels == 1 and .input.aspect_ratio == "1:1" and (.input.instructions.text | contains("OWN-QWEN-PROMPT") and (contains("PUBLIC-WORDS") | not))' <<< "$(call clean)" || fail "qwen's call: $(call clean)"
pic=$(file_of clean image); cp "$pic" "$OUT/frames/clean.${pic##*.}"; s=$(size "$pic"); say "clean: $s"
[ "$s" = 1024x1024 ] || fail "1:1 at 1 MP made $s"

say "qwen-image-2/image: two pictures, each read with its words"
drop two "$PK/qwen-image-2/image" '{"instructions": "<art> and <clean> side by side on one sheet.", "aspect_ratio": "16:9", "megapixels": 1}' image
accept two
J '.input.instructions.references | length == 2 and .[0].key == "art" and .[1].key == "clean" and (.[1].description | contains("PUBLIC-WORDS"))' <<< "$(call two)" || fail "two pictures' call: $(call two)"
pic=$(file_of two image); cp "$pic" "$OUT/frames/two.${pic##*.}"; say "two: $(size "$pic")"

say "birefnet/remove_background: called with its file by its typed input"
drop cut "$PK/birefnet/remove_background" '{"image": "<art>"}' image
accept cut
J '(.target | test("/remove_background(@|$)")) and (.input.image | startswith("sha256:")) and (.input | keys == ["image"])' <<< "$(call cut)" || fail "birefnet's call: $(call cut)"
pic=$(file_of cut image); cp "$pic" "$OUT/frames/cut.${pic##*.}"; say "cut: $(size "$pic")"

say "minimax-h3/generate: a shot with beats, reading what was accepted"
drop shot "$PK/minimax-h3/generate" '{"summary": "<clean> stands in a quiet studio.", "beats": [{"description": "<clean> turns slowly toward the camera while soft light moves across the wall.", "duration": 5}], "soundscape": "Soft room tone."}' video
accept shot
J '(.target | test("/generate(@|$)")) and (.input | (has("turbo") | not) and (has("prompt") | not) and .beats[0].duration == 5 and .soundscape.text == "Soft room tone." and (.summary.references[0].key == "clean") and (.summary.references[0].description | contains("PUBLIC-WORDS")) and (tostring | contains("OWN-QWEN-PROMPT") | not))' <<< "$(call shot)" || fail "h3's call: $(call shot)"
vid=$(file_of shot video); keep_frames shot "$vid"; s=$(size "$vid"); d=$(seconds "$vid"); say "shot: $s, $d s"
[ "$s" = 1344x768 ] || fail "the shot is $s"
awk -v d="$d" 'BEGIN{exit !(d > 4.5 && d < 6.5)}' || fail "a 5 s beat made $d s"
ffprobe -v error -select_streams a -show_entries stream=codec_type -of csv=p=0 "$vid" | grep -q audio || fail "the shot has no sound"

say "minimax-h3/generate: first and last frames, nothing said of its sound, so N/A; the text H3 reads is in its run's log"
drop framed "$PK/minimax-h3/generate" '{"beats": [{"description": "The subject turns slowly toward the camera while soft light moves across the wall.", "duration": 5}], "first_frame": "<clean>", "last_frame": "<two>"}' video
accept framed
J '.input | (.first_frame | startswith("sha256:")) and (.last_frame | startswith("sha256:")) and (has("soundscape") | not) and (has("music") | not)' <<< "$(call framed)" || fail "framed's call: $(call framed)"
log framed > "$OUT/framed-log.txt" 2>&1 || fail "framed's log"
grep -q 'h3 segment prompt' "$OUT/framed-log.txt" || fail "framed's log has no prompt"
grep -q 'N/A' "$OUT/framed-log.txt" || fail "a silent shot's text doesn't say N/A: $(grep -m1 'h3 segment prompt' "$OUT/framed-log.txt" | cut -c1-600)"
! grep -q 'reads as silence' "$OUT/framed-log.txt" || fail "a silent shot is warned of"
vid=$(file_of framed video); keep_frames framed "$vid"; say "framed: $(size "$vid"), $(seconds "$vid") s"

say "minimax-h3/generate: one beat, no summary, as written: MiniMax's own form reaches the model untouched"
drop asis "$PK/minimax-h3/generate" '{"beats": [{"description": "[Shot 1] A paper boat drifts down a rain gutter.\nAt 00:03.000, it tips over a grate.", "duration": 6}]}' video
accept asis
log asis > "$OUT/asis-log.txt" 2>&1 || fail "asis's log"
grep -q 'A paper boat drifts' "$OUT/asis-log.txt" || fail "as written: $(grep -m1 'h3 segment prompt' "$OUT/asis-log.txt" | cut -c1-600)"
vid=$(file_of asis video); keep_frames asis "$vid"; say "as written: $(size "$vid"), $(seconds "$vid") s"

say "minimax-h3/upscale: its own tool, an H3 take and a resolution"
drop up "$PK/minimax-h3/upscale" '{"video": "<shot>", "to": "1080p"}' video
accept up
J '(.target | test("/upscale(@|$)")) and .input.to == "1080p" and (.input.video.file | startswith("sha256:")) and (.input.video.latents | startswith("sha256:")) and (.input | has("prompt") | not)' <<< "$(call up)" || fail "upscale's call: $(call up)"
vid=$(file_of up video); keep_frames up "$vid"; s=$(size "$vid"); say "up: $s, $(seconds "$vid") s"
[ "${s#*x}" -ge 1080 ] || fail "upscaled to $s"

say "a beat past the package's limit is clamped and said, with the tool's own number"
run_page tools > "$OUT/tools.json" 2> "$OUT/tools.err" || fail "the tools: $(tail -3 "$OUT/tools.err")"
J --arg t "$PK/minimax-h3/generate" '.tools[] | select(.id == $t) | .fields[] | select(.field == "beats") | .type == "group" and .list and .min_items == 1 and (.members[] | select(.field == "duration") | .max == 15)' "$OUT/tools.json" || fail "the beats field: $(jq -c --arg t "$PK/minimax-h3/generate" '.tools[] | select(.id == $t)' "$OUT/tools.json")"
J --arg t "$PK/minimax-h3/generate" '.tools[] | select(.id == $t) | [.fields[] | .field][:4] == ["summary", "beats", "soundscape", "music"]' "$OUT/tools.json" || fail "h3's box: $(jq -c --arg t "$PK/minimax-h3/generate" '.tools[] | select(.id == $t) | .fields | map(.field)' "$OUT/tools.json")"
J --arg t "$PK/qwen-image-2/image" '.tools[] | select(.id == $t) | .fields[] | select(.field == "megapixels") | .type == "choice" and .options == [1, 2, 4]' "$OUT/tools.json" || fail "the megapixels field"

say "a file like lab, as the upgrade writes it"
run_page yaml > "$OUT/lab.yaml" 2> "$OUT/lab-export.err" || fail "the project as YAML: $(tail -3 "$OUT/lab-export.err")"
cat >> "$OUT/lab.yaml" <<YAML
  lab_sheet:
    tool: $PK/qwen-image-2/image
    instructions: This is an RGBA image with transparency. A bronze clockwork automaton, waist up, from <art>, shown from the front on one sheet, with no text. The image has alpha channel and the background is transparent.
    aspect_ratio: "3:2"
    megapixels: 1
    asset_type: character
  video_style:
    text: High-quality Unreal Engine graphics, realistic and detailed.
  lab_shot:
    tool: $PK/minimax-h3/generate
    aspect_ratio: "16:9"
    summary: <video_style> <lab_sheet> walks through a workshop at dusk.
    soundscape: Quiet gears.
    beats:
      - duration: 5
        description: The camera follows <lab_sheet> as it crosses the workshop.
YAML
run_page yaml "$OUT/lab.yaml" > "$OUT/lab-read.txt" 2>&1 || fail "lab's file: $(tail -3 "$OUT/lab-read.txt")"
render lab_shot --wait lab_sheet,lab_shot || fail "lab's shape: $(tail -c 500 "$OUT/render-lab_shot.txt")"
J '(.target | test("/image(@|$)")) and .input.aspect_ratio == "3:2" and .input.megapixels == 1' <<< "$(call lab_sheet)" || fail "the image's call: $(call lab_sheet)"
J '(.target | test("/generate(@|$)")) and .input.beats[0].duration == 5 and (.input.summary.text | contains("High-quality Unreal"))' <<< "$(call lab_shot)" || fail "the shot's call: $(call lab_shot)"
pic=$(file_of lab_sheet image); cp "$pic" "$OUT/frames/lab-sheet.${pic##*.}"
vid=$(file_of lab_shot video); keep_frames lab "$vid"; say "lab shape: $(size "$pic"), $(size "$vid") $(seconds "$vid") s"

say "Pick up from here: a longer shot, kept to a join and the rest made again"
drop long "$PK/minimax-h3/generate" '{"summary": "<clean> stands in a quiet studio.", "beats": [{"description": "<clean> looks left.", "duration": 8}, {"description": "<clean> looks right.", "duration": 8}], "soundscape": "Soft room tone."}' video
accept long
kept=$(file_of long video)
# Frame 141 of take 1 (5.9 s), on the frames H3's pickup lands on: 124, then every 17.
render long --pickup '<long.1>' 141 || fail "the pickup: $(tail -c 500 "$OUT/render-long.txt")"
J '.input.pickup | .out_frame == 141 and (.take.file | startswith("sha256:")) and (.take.latents | startswith("sha256:"))' <<< "$(call long)" || fail "the kept call: $(call long)"
vid=$(file_of long video); keep_frames take "$vid"; say "kept: $(size "$vid"), $(seconds "$vid") s"
run=$(newest_run long)
kept_s=$(cozy --json run watch "$run" --once | jq -r '.result.kept_s // empty')
[ -n "$kept_s" ] || fail "run $run says no kept_s: it didn't resume from the latents"
held "$kept" "$vid" "$(awk -v s="$kept_s" 'BEGIN { printf "%d", s * 24 + 0.5 }')"

say "minimax-h3/edit: a plate, and a reference video and its sound"
drop plated "$PK/minimax-h3/edit" '{"plate": "<shot>", "edit": "vfx_edit", "beats": [{"description": "<clean> walks on as snow begins to fall.", "duration": 5}]}' video
accept plated
J '(.target | test("/edit(@|$)")) and (.input.plate | startswith("sha256:")) and .input.edit == "vfx_edit" and (.input.beats[0].description.references[0].key == "clean")' <<< "$(call plated)" || fail "the plate's call: $(call plated)"
vid=$(file_of plated video); keep_frames plated "$vid"; say "plated: $(size "$vid"), $(seconds "$vid") s"

(cd "$RUN_PROJECT" && cozy --json --full status) > "$OUT/status.json"
run_page yaml > "$OUT/project.yaml"
say "PASS: each tool from drop to accept; judge $OUT/frames"
