# tool-functions (cl-774, "A tool is a function"): each tool from dropping it to accepting its output, on one rental driven from a
# runner, the way the page does it: POST assets {tool, fields}, render, approve. A shape like `lab`'s, naming only a package, goes too.
# Packages are local code from a packages_refs checkout named `pk`, since no release has these adapters yet; the session's cozy carries
# its TensorD to the pod, and `runtime` the Runtime that reads `functions=`. A completed run is no pass: each output is measured, its
# frames kept for a person to judge in $OUT/frames, and each call's input is checked against what crossed its mentions. One rental
# (a 4090). params:
#   runtime     the Runtime wheel with functions=, https://…#sha256=<hex> (default: the session's RUNTIME_WHEEL, from rc/<x>)
#   traffic_gb  GB the provider bills (110)
#   caption     also ask a local qwen3.5 for a description, 9 GB more ("true")
. "$(dirname "$0")/lib.sh"
URL=$(param runtime) PK=$PACKAGES/pk CAPTION=$(param caption true)
for p in qwen-image-2 minimax-h3 birefnet seedvr2 qwen3.5; do [ -f "$PK/$p/pyproject.toml" ] || fail "no packages_refs checkout pk/$p"; done
grep -q 'functions=\["upscale"\]' "$PK/minimax-h3/h3.py" || fail "pk is not rc/web-ui: minimax-h3 has no adapter per function"
mkdir -p "$OUT/wheel" "$OUT/frames"
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}")
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
else
  W=${RUNTIME_WHEEL:-}
  [ -n "$W" ] || fail "no Runtime with functions=: name \`runtime\`, or dispatch from rc/<x>"
fi
command -v ffprobe > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
[ -f "$REF/seed.jpg" ] || fail "no reference image"
J() { jq -e "$@" > /dev/null; }
P=$OUT/nodes
cozy project new "$P" > /dev/null 2>&1 || fail "project new"
printf 'version: 3\nassets: {}\n' > "$P/project.yaml"
cp "$REF/seed.jpg" "$OUT/art.jpg"
cozy project -p "$P" import "$OUT/art.jpg" art > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
ID=$(cozy --json project -p "$P" status | jq -r .id)
lock=$HOME/.cozy/calcifer/calcifer.lock.holder
ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock")
api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }
# drop <name> <tool> <fields json> <kind>: what dropping a tool writes, with its kind from its output.
drop() {
  local made
  made=$(api -X POST -H "Idempotency-Key: drop-$1-$RANDOM$RANDOM" -d "$(jq -n --arg n "$1" --arg t "$2" --argjson f "$3" '{name: $n, tool: $t, fields: $f}')" "http://$ADDR/v1/projects/$ID/assets")
  J --arg n "$1" '.name == $n' <<< "$made" || fail "drop $1: $made"
  J --arg n "$1" --arg k "$4" '.assets[] | select(.name == $n) | .kind == $k' <<< "$(cozy --json project -p "$P" status)" || fail "$1's kind isn't $4, what $2 outputs"
}
# accept <name>: render it on the pod, take its newest version, and say which version that is in its files.
accept() {
  step "render-$1" project -p "$P" render "$1" --new --on "$NAME" || fail "$1: $(tail -c 500 "$OUT/render-$1.txt")"
  local n
  n=$(cozy --json project -p "$P" show "$1" | jq -r '.versions[-1].number')
  step "approve-$1" project -p "$P" approve "$1" "$n" || fail "approve $1: $(tail -c 300 "$OUT/approve-$1.txt")"
  J --arg n "$1" '.assets[] | select(.name == $n) | .pinned' <<< "$(cozy --json project -p "$P" status)" || fail "$1 isn't accepted"
}
call() { cozy --json --full run show "$(cozy --json project -p "$P" show "$1" | jq -r '.versions[-1].runs[-1]')" --once | jq -c .request; }
file_of() { cozy --json project -p "$P" show "$1" | jq -r --arg f "$2" --arg P "$P" '$P + "/" + .versions[-1].files[$f].path'; }
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$1"; }
seconds() { ffprobe -v error -show_entries format=duration -of csv=p=0 "$1"; }
keep_frames() { ffmpeg -v error -y -i "$2" -vf "select='eq(n,0)+eq(n,60)+eq(n,120)'" -vsync 0 "$OUT/frames/$1-%d.png"; }

quote "$(param traffic_gb 110)" 3600
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

say "qwen-image-2/generate_turbo: an image edit reading a picture, its description crossing and its prompt not"
drop clean "$PK/qwen-image-2/generate_turbo" '{"instructions": "OWN-QWEN-PROMPT Keep the subject of <art> exactly as it is, on a plain light grey studio background.", "description": "PUBLIC-WORDS the subject on a plain grey background.", "aspect_ratio": "1:1", "megapixels": 1}' image
accept clean
J '(.target | test("/generate_turbo(@|$)")) and (.input.pictures[0].name == "art") and .input.megapixels == 1 and .input.aspect_ratio == "1:1" and (.input.prompt | contains("OWN-QWEN-PROMPT") and (contains("PUBLIC-WORDS") | not))' <<< "$(call clean)" || fail "qwen's call: $(call clean)"
pic=$(file_of clean image); cp "$pic" "$OUT/frames/clean.${pic##*.}"; s=$(size "$pic"); say "clean: $s"
[ "$s" = 1024x1024 ] || fail "1:1 at 1 MP made $s"

say "birefnet/remove_background: no adapter, called with its file by its typed input"
drop cut "$PK/birefnet/remove_background" '{"image": "<art>"}' image
accept cut
J '(.target | test("/remove_background(@|$)")) and (.input.image | startswith("sha256:")) and (.input | keys == ["image"])' <<< "$(call cut)" || fail "birefnet's call: $(call cut)"
pic=$(file_of cut image); cp "$pic" "$OUT/frames/cut.${pic##*.}"; say "cut: $(size "$pic")"

say "minimax-h3/generate_turbo: a shot with beats, reading what was accepted"
drop shot "$PK/minimax-h3/generate_turbo" '{"summary": "<clean> stands in a quiet studio.", "beats": [{"description": "<clean> turns slowly toward the camera while soft light moves across the wall.", "duration": 5}], "soundscape": "Soft room tone."}' video
accept shot
J '(.target | test("/generate_turbo(@|$)")) and (.input | (has("turbo") | not) and .duration_s == 5 and (.references[0].name == "clean") and (.references[0].description | contains("PUBLIC-WORDS")) and (tostring | contains("OWN-QWEN-PROMPT") | not))' <<< "$(call shot)" || fail "h3's call: $(call shot)"
vid=$(file_of shot video); keep_frames shot "$vid"; s=$(size "$vid"); d=$(seconds "$vid"); say "shot: $s, $d s"
[ "$s" = 1344x768 ] || fail "the shot is $s"
awk -v d="$d" 'BEGIN{exit !(d > 4.5 && d < 6.5)}' || fail "a 5 s beat made $d s"
ffprobe -v error -select_streams a -show_entries stream=codec_type -of csv=p=0 "$vid" | grep -q audio || fail "the shot has no sound"

say "minimax-h3/upscale: its own tool, a video and a resolution"
drop up "$PK/minimax-h3/upscale" '{"video": "<shot>", "resolution": "1080p"}' video
accept up
J '(.target | test("/upscale(@|$)")) and .input.resolution == "1080p" and (.input.latents | startswith("sha256:")) and (.input | has("prompt") | not)' <<< "$(call up)" || fail "upscale's call: $(call up)"
vid=$(file_of up video); keep_frames up "$vid"; s=$(size "$vid"); say "up: $s, $(seconds "$vid") s"
[ "${s#*x}" -ge 1080 ] || fail "upscaled to $s"

say "a beat past the package's limit is clamped and said, with the tool's own number"
api --get --data-urlencode "project=$ID" "http://$ADDR/v1/tools" > "$OUT/tools.json"
J --arg t "$PK/minimax-h3/generate_turbo" '.tools[] | select(.id == $t) | .controls[] | select(.field == "beats") | .max_duration == 15' "$OUT/tools.json" || fail "the beats control: $(jq -c --arg t "$PK/minimax-h3/generate_turbo" '.tools[] | select(.id == $t)' "$OUT/tools.json")"
J --arg t "$PK/qwen-image-2/generate_turbo" '.tools[] | select(.id == $t) | .controls[] | select(.field == "megapixels") | .type == "range" and .min == 1 and .max == 4' "$OUT/tools.json" || fail "the megapixels control"

say "a file like lab, as the upgrade writes it"
cat >> "$P/project.yaml" <<YAML
  lab_sheet:
    tool: $PK/qwen-image-2/generate_turbo
    instructions: A bronze clockwork automaton, waist up, from <art>, shown from the front on one sheet, with no text.
    aspect_ratio: "3:2"
    megapixels: 1
    transparent: true
    asset_type: character
  lab_shot:
    tool: $PK/minimax-h3/generate
    style: High-quality Unreal Engine graphics, realistic and detailed
    aspect_ratio: "16:9"
    summary: <lab_sheet> walks through a workshop at dusk.
    soundscape: Quiet gears.
    beats:
      - duration: 5
        description: The camera follows <lab_sheet> as it crosses the workshop.
YAML
step render-lab project -p "$P" render lab_shot --on "$NAME" || fail "lab's shape: $(tail -c 500 "$OUT/render-lab.txt")"
J '(.target | test("/generate_turbo(@|$)")) and .input.aspect_ratio == "3:2" and .input.megapixels == 1' <<< "$(call lab_sheet)" || fail "the image's call: $(call lab_sheet)"
J '(.target | test("/generate(@|$)")) and .input.duration_s == 5' <<< "$(call lab_shot)" || fail "the shot's call: $(call lab_shot)"
pic=$(file_of lab_sheet image); cp "$pic" "$OUT/frames/lab-sheet.${pic##*.}"
vid=$(file_of lab_shot video); keep_frames lab "$vid"; say "lab shape: $(size "$pic"), $(size "$vid") $(seconds "$vid") s"

if [ "$CAPTION" = true ]; then
  say "Caption: what the package says to ask, nothing written"
  cat >> "$P/project.yaml" <<YAML
  names_qwen35:
    tool: $PK/qwen3.5
YAML
  before=$(sha256sum "$P/project.yaml")
  timeout 1800 curl -sS -H "Authorization: Bearer $TOKEN" -X POST -d "$(jq -n --arg o "$NAME" '{on: $o}')" "http://$ADDR/v1/projects/$ID/assets/cut/caption" > "$OUT/caption.json"
  J '.text | length > 10' "$OUT/caption.json" || fail "caption: $(head -c 400 "$OUT/caption.json")"
  [ "$before" = "$(sha256sum "$P/project.yaml")" ] || fail "caption wrote to the file"
  say "caption: $(jq -r .text "$OUT/caption.json" | cut -c1-300)"
fi
cozy --json --full project -p "$P" status > "$OUT/status.json"
cp "$P/project.yaml" "$P/project.lock" "$OUT/"
say "PASS: each tool from drop to accept; judge $OUT/frames"
