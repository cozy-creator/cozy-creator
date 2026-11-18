# h3-edit-sound (se-163, proof only): minimax-h3's edit keeps, regenerates or makes new its plate's sound, and is its
# plate's frames, on one rental, through the page's own flow (e2e/page.py). Packages from packages_refs checkouts: `pk`
# (the change), `rc` (rc/0.27.0, before it) and `pc` (a probe: regenerate marked `partially_copy`).
# The clips are public, fetched here from Wikimedia Commons: NASA's "Astronaut Scott Kelly Speaks Out Against Bullying"
# (public domain) from 6.4 s for 12.6 s, 302 slots at 24 fps from 29.97 (`kelly`); NASA's "A Message of Resilience from
# the International Space Station" (public domain) from 8.25 s for 59.6 s, 1430 slots from 59.94 fps, Opus (`crew`); and
# Sintel from the Durian Project's cover (CC BY 3.0, Blender Foundation) as the character swapped in.
# Before renting, at $0: the planner over every plate length (fixtures/h3-edit-sound/plan.py, in minimax-h3's own locked
# env) and the analysis rehearsed on the plates. Cells, one seed, Turbo, character swap:
#   before            rc's edit of kelly (today: H3's own sound, the grid's tail past the plate)
#   new keep regen    pk's edit of kelly, each `audio`
#   regen-pc          pc's regenerate of kelly
#   long-keep long-regen long-new   pk's edit of crew, each `audio`
# The pod ends before the analysis (analyze.py): frames against the plate, keep's PCM, lip sync, the end, seams, voice.
# params:
#   runtime    the Runtime wheel, https://…#sha256=<hex>
#   cells      the cells to run, space-separated (default all, in the order above)
#   plan       false: skip the planner check
#   need_s     seconds the cap must buy (7200)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/h3-edit-sound" && pwd) || fail "no fixtures"
PK=$PACKAGES/pk RCT=$PACKAGES/rc PC=$PACKAGES/pc
for t in "$PK" "$RCT" "$PC"; do [ -f "$t/minimax-h3/pyproject.toml" ] || fail "no packages_refs checkout ${t#"$PACKAGES"/}"; done
grep -q 'def plate_sound' "$PK/minimax-h3/minimax_h3/shot.py" || fail "pk has no plate_sound: not se-163's branch"
CELLS=$(param cells "before new keep regen regen-pc long-keep long-regen long-new")
for tool in ffmpeg ffprobe curl jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
URL=$(param runtime)
[[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
mkdir -p "$OUT/wheel" "$OUT/media" "$OUT/frames"
W=$OUT/wheel/$(basename "${URL%%#*}" | sed 's/%2B/+/g')
curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
[ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"

# The clips, public, and the plates cut from them here.
fetch() { curl -fsSL --retry 5 -A "cozy-release-gate/1.0" "$2" -o "$OUT/media/$1" || fail "fetch $1"; }
fetch kelly.webm "https://upload.wikimedia.org/wikipedia/commons/c/ce/Astronaut_Scott_Kelly_Speaks_Out_Against_Bullying.webm"
fetch crew.webm "https://upload.wikimedia.org/wikipedia/commons/4/43/A_Message_of_Resilience_from_the_International_Space_Station.webm"
fetch sintel-cover.jpg "https://upload.wikimedia.org/wikipedia/commons/6/69/Sintel_Cover_Durian_Project.jpg"
M=$OUT/media
ffmpeg -v error -y -ss 6.4 -i "$M/kelly.webm" -t 12.6 -c:v libx264 -crf 18 -pix_fmt yuv420p -c:a aac -b:a 160k \
  -movflags +faststart "$M/kelly.mp4" || fail "kelly plate"
ffmpeg -v error -y -ss 8.25 -i "$M/crew.webm" -t 59.6 -c:v libx264 -crf 20 -pix_fmt yuv420p -c:a copy \
  -movflags +faststart "$M/crew.mp4" || fail "crew plate"
ffmpeg -v error -y -ss 8.25 -i "$M/crew.webm" -t 12.6 -c:v libx264 -crf 20 -pix_fmt yuv420p -c:a copy \
  -movflags +faststart "$M/opus.mp4" || fail "opus plate"
ffmpeg -v error -y -i "$M/sintel-cover.jpg" -vf "crop=748:1000:330:250" -q:v 2 "$M/sintel.jpg" || fail "sintel"
for p in kelly crew opus; do
  say "plate $p: $(ffprobe -v error -show_entries stream=codec_name,r_frame_rate,start_time,duration -of compact=p=0:nk=1 "$M/$p.mp4" | paste -sd' ')"
done

if [ "$(param plan true)" = true ]; then
  # minimax-h3's locked env, Torch beside it as scripts/triton.sh puts the worker image's (a lock carries none, cl-715), on CPU.
  say "the planner over every plate length, in minimax-h3's locked env (CPU)"
  E=$OUT/plan-env
  { (cd "$PK/minimax-h3" && UV_PROJECT_ENVIRONMENT=$E uv sync -q --frozen --no-dev --python 3.12) &&
    VIRTUAL_ENV=$E uv pip install -q --index-url https://download.pytorch.org/whl/cpu torch==2.15.0 torchvision==0.29.2; } \
    > "$OUT/plan-sync.log" 2>&1 || fail "the planner's env: $(tail -c 500 "$OUT/plan-sync.log")"
  "$E/bin/python" -I "$FIX/plan.py" "$OUT/plan.json" > "$OUT/plan.txt" 2>&1; code=$?
  tail -25 "$OUT/plan.txt" | tee -a "$R"
  [ "$code" = 0 ] || fail "the planner check: $(tail -c 600 "$OUT/plan.txt")"
  rm -rf "$E"
fi
AN=(uv run -q --no-project --python 3.12 --with av==19.0.1 --with "numpy<2" --with mediapipe==0.10.18 --with pillow
    --with faster-whisper==1.1.1 --with librosa==0.10.2.post1 python -I "$FIX/analyze.py")
say "the analysis rehearsed on the plates (each against itself)"
jq -n --arg k "$M/kelly.mp4" --arg c "$M/crew.mp4" \
  '[{name: "plate-kelly", path: $k, plate: $k, mode: "keep"}, {name: "plate-crew", path: $c, plate: $c, mode: "keep", long: true, seams: [362, 702]}]' \
  > "$OUT/rehearse.json"
"${AN[@]}" "$OUT/rehearse.json" "$OUT/rehearse" > "$OUT/rehearse.txt" 2>&1 || fail "the analysis rehearsal: $(tail -c 600 "$OUT/rehearse.txt")"
tail -4 "$OUT/rehearse.txt" | cut -c1-600 | tee -a "$R"
for t in "$PK" "$RCT" "$PC"; do kernels "$t/minimax-h3"; done

quote "$(param need_s 7200)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

run_page import "$M/kelly.mp4" --name kelly > "$OUT/import-kelly.txt" 2>&1 || fail "import kelly: $(cat "$OUT/import-kelly.txt")"
run_page import "$M/crew.mp4" --name crew > "$OUT/import-crew.txt" 2>&1 || fail "import crew: $(cat "$OUT/import-crew.txt")"
run_page import "$M/opus.mp4" --name opus > "$OUT/import-opus.txt" 2>&1 || fail "import opus: $(cat "$OUT/import-opus.txt")"
run_page import "$M/sintel.jpg" --name sintel --description "Sintel, a young woman with short red hair, a grey laced top and leather arm wraps, a small dragon on her shoulder" \
  > "$OUT/import-sintel.txt" 2>&1 || fail "import sintel: $(cat "$OUT/import-sintel.txt")"
SWAP='<sintel> takes the place of the man in the plate, keeping its scene, camera and his whole performance: pose, gestures, head motion, gaze, speech, lip movement and expressions.'
CREW='<sintel> takes the place of the man in the middle of the crew, keeping the scene, the camera, the rest of the crew and his whole performance: pose, gestures, head motion, gaze, speech, lip movement and expressions.'
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r '.versions[-1].files.video.path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 900 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
echo '[]' > "$OUT/manifest.json"
# cell <name> <tree> <plate> <audio|-> <summary> <beat>: an edit box rendered on the pod; its video, record and log kept.
cell() {
  local name=$1 tree=$2 plate=$3 audio=$4 fields n v t0=$SECONDS
  fields=$(jq -cn --arg p "<$plate>" --arg s "$5" --arg b "$6" --arg a "$audio" \
    '{plate: $p, edit: "character_swap", summary: $s, seed: 163163, beats: [{duration: 15, description: $b}]} + (if $a == "-" then {} else {audio: $a} end)')
  run_page add "$name" --tool "$tree/minimax-h3/edit" --fields "$fields" > "$OUT/add-$name.txt" 2>&1 || fail "add $name: $(cat "$OUT/add-$name.txt")"
  say "render $name ($(basename "$tree") $plate audio=$audio)"
  if ! run_page render --on "$NAME" "$name" > "$OUT/render-$name.txt" 2>&1; then
    say "FAIL $name: $(tail -c 600 "$OUT/render-$name.txt")"; RC=1; return 1
  fi
  n=$(newest_run "$name")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$name.json" 2>&1
  (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/$name.log" 2>&1
  v=$(file_of "$name") || { say "FAIL $name made no video"; RC=1; return 1; }
  cp "$v" "$OUT/$name.mp4"
  say "$name: run $n in $((SECONDS - t0)) s, $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$OUT/$name.mp4") frames, \
$(grep -o 'h3 shot segments[^"]*' "$OUT/$name.log" | head -1 | cut -c1-300); warnings $(jq -c '.result.warnings // []' "$OUT/$name.json" | cut -c1-400)"
  jq --arg n "$name" --arg p "$M/$plate.mp4" --arg v "$OUT/$name.mp4" --arg a "$audio" --argjson s "$(jq -c '.result.seams // []' "$OUT/$name.json")" \
    --argjson l "$([ "$plate" = crew ] && echo true || echo false)" \
    '. + [{name: $n, path: $v, plate: $p, mode: (if $a == "-" then "before" elif $a == "regenerate" then "regenerate" else $a end), seams: $s, long: $l}]' \
    "$OUT/manifest.json" > "$OUT/manifest.next" && mv "$OUT/manifest.next" "$OUT/manifest.json"
}
for c in $CELLS; do
  case $c in
    before) cell before "$RCT" kelly - "$SWAP" '<sintel> speaks to the camera in front of the rocket.' ;;
    new) cell new "$PK" kelly new "$SWAP" '<sintel> speaks to the camera in front of the rocket.' ;;
    keep) cell keep "$PK" kelly keep "$SWAP" '<sintel> speaks to the camera in front of the rocket.' ;;
    regen) cell regen "$PK" kelly regenerate "$SWAP" '<sintel> speaks to the camera in front of the rocket.' ;;
    regen-pc) cell regen-pc "$PC" kelly regenerate "$SWAP" '<sintel> speaks to the camera in front of the rocket.' ;;
    long-keep) cell long-keep "$PK" crew keep "$CREW" '<sintel> talks to the camera with the crew in the space station.' ;;
    long-regen) cell long-regen "$PK" crew regenerate "$CREW" '<sintel> talks to the camera with the crew in the space station.' ;;
    long-new) cell long-new "$PK" crew new "$CREW" '<sintel> talks to the camera with the crew in the space station.' ;;
    keep-opus) cell keep-opus "$PK" opus keep "$CREW" '<sintel> talks to the camera with the crew in the space station.' ;;
    *) fail "no cell $c" ;;
  esac
done

# The pod ends here, before the analysis: nothing below needs it.
for r in "${RIDS[@]}"; do
  until gone "$r"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; sleep 10; done
  say "GONE $r before the analysis; billed $(billed "$r")"
done
say "analysis"
"${AN[@]}" "$OUT/manifest.json" "$OUT/analysis" > "$OUT/analysis.txt" 2>&1 || { say "FAIL analysis: $(tail -c 600 "$OUT/analysis.txt")"; RC=1; }
cut -c1-900 "$OUT/analysis.txt" | tee -a "$R"
exit "$RC"
