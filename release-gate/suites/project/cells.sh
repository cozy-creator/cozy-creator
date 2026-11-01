# Suite `project` (xs-139, cl-716): `cozy project render` on fixture projects, the walk in this computer's Calcifer (the
# candidate cozy), the runs on $M. Each cell is a project folder under $OUT, judged from its project.lock and assets:
# - project-split: a 10 s shot whose write at 5 s a later beat reads: two calls, the second continuing the first
#   (`continue_from`, the first's latents), the written frame kept, the join smooth; project-upscale: its upscale from
#   those latents, at 1080p and the shot's aspect, and edit.mp4 at the 1920x1080 canvas.
# - project-plate: a shot with `plate:` follows it frame by frame (guide-equal's bar).
# - project-voice: a character with `voice:` speaks; the voice goes with the request and the video has sound.
# - project-stills: a place and its setup at the canvas aspect, at 16:9 and again at 9:16; a character cleaned.
# - project-local: a node rendered `with: ./still.py/draw`, local code made ready by Calcifer.
# - project-refusals: a plate to a function that takes none, and an option that sets the canvas, refused before any run.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# A checkout's ./pkg/fn names the gate's folder; a project reads `with:` from its own.
abs() { [[ $1 == ./* ]] && echo "$OUT/${1#./}" || echo "$1"; }
H3GEN=$(abs "$(pkg minimax-h3 generate)") H3UP=$(abs "$(pkg minimax-h3 upscale)") STILL=$(abs "$(pkg qwen-image-2 generate)")
CLEAN=$(abs "$(pkg birefnet remove_background)") SDXL=$(abs "$(pkg sdxl generate)")
want() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* || $ONLY == *",project,"* ]]; }
# project <cell>: a fresh folder for it, its project.yaml from stdin, the gate's pictures beside it.
project() {
  local P=$OUT/$1; rm -rf "$P"; mkdir -p "$P"; cat > "$P/project.yaml"
  nice -n 19 ffmpeg -v error -y -i "${K:-$REF}" "$P/hero.png"; nice -n 19 ffmpeg -v error -y -i "$REF" "$P/place.png"
  echo "$P"
}
render() { local c=$1 P=$2; shift 2; "$CLI" project -p "$P" render "$@" --on "$M" > "$c.log" 2>&1; }
# lockq <folder> <python expression of lock>
lockq() { python3 -c "import json, sys; lock = json.load(open(sys.argv[1] + '/project.lock')); print($2)" "$1" 2>/dev/null; }
# approved <folder> <node>: the approved take's or version's record, as JSON; its primary asset (`.assets.video`,
# `.clean` or `.image`) is the one project.lock approves.
approved() { lockq "$1" "json.dumps(next((v for v in lock['nodes'].get('$2', {}).get('versions', []) if (v['assets'].get('video') or v['assets'].get('clean') or v['assets'].get('image'))['path'] == lock['nodes']['$2'].get('approved')), {}))"; }
PRIMARY='.assets.video.path // .assets.clean.path // .assets.image.path // empty' 
aspect() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1" | awk -F, '{ printf "%.4f", $1 / $2 }'; }
near() { awk -v a="$1" -v b="$2" 'BEGIN { d = a / b - 1; print (d < 0 ? -d : d) <= 0.03 ? "PASS" : "FAIL" }'; }
SEAMBAR='if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end'

if want project-split || want project-upscale; then
  P=$(project project-split <<YAML
type: video
style: "naturalistic photo, warm late-afternoon light, handheld camera"
aspect_ratio: "16:9"
resolution: 1080p
defaults: {shot: {with: $H3GEN}, upscale: {with: $H3UP}, clean: {with: $CLEAN}}
entities:
  - {name: hero, kind: character, description: "$CHAR", image: hero.png}
  - {name: yard, kind: environment, description: "$SCENE", image: place.png}
world_states:
  - {name: wall, init: "<yard>'s long wall, bare.", blocking: "a stepladder against the wall", image: place.png}
shots:
  - name: mural
    prompt: "<hero> paints a sign on the long wall of <yard>."
    duration_s: 10
    render: {options: {seed: $((R + 300))}}
    writes: [{state: wall, at: 5}]
    beats:
      - {at: 0, prompt: "<hero> rolls bright orange paint across the wall, painting big letters that read OPEN."}
      - {at: 6, prompt: "<hero> steps back from <wall>, where the orange letters read OPEN, and says \"Done.\""}
YAML
)
  render project-split "$P" all --preview || expect project-split FAIL "render all --preview: $(tail -c 300 project-split.log)"
  v=$(approved "$P" mural)
  f=$(jq -r "$PRIMARY" <<< "$v")
  if [ -z "$f" ]; then expect project-split FAIL "no version of mural: $(tail -c 300 project-split.log)"
  else
    calls=$(jq -r '.runs // [] | map(tostring) | join(" ")' <<< "$v")
    expect project-split "$(jq -r 'if (.runs | length) >= 2 and .assets.latents != null and .writes.wall != null then "PASS" else "FAIL" end' <<< "$v")" \
      "runs $calls, latents $(jq -r '.assets.latents.path // "none"' <<< "$v"), wall $(jq -r '.writes.wall.path // "none"' <<< "$v"), made by $(jq -r '.made_by.with' <<< "$v")"
    r=$("${FR[@]}" seams "$P/$f" --at 5 2>&1 | tail -1)
    expect project-split-seam "$(jq -r "$SEAMBAR" <<< "$r" 2>/dev/null || echo FAIL)" "join at 5 s: $r"
  fi
  if want project-upscale && [ -n "$f" ]; then
    render project-upscale "$P" mural.upscale || expect project-upscale FAIL "render mural.upscale: $(tail -c 300 project-upscale.log)"
    u=$(jq -r "$PRIMARY" <<< "$(approved "$P" mural.upscale)")
    if [ -z "$u" ]; then expect project-upscale FAIL "no upscale: $(tail -c 300 project-upscale.log)"
    else
      h=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$P/$u")
      expect project-upscale "$([ "$h" = 1080 ] && [ "$(near "$(aspect "$P/$u")" "$(aspect "$P/$f")")" = PASS ] && echo PASS || echo FAIL)" \
        "upscale $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$u"), shot $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$f")"
      grep -q 'latents' project-upscale.log || "$CLI" run show "$(lockq "$P" "next(v for v in lock['nodes']['mural.upscale']['versions'])['runs'][-1]")" --once 2>/dev/null | grep -q latents \
        || expect project-upscale-latents FAIL "the upscale was not sent the shot's latents"
      render project-edit "$P" all
      e=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/edit.mp4" 2>/dev/null)
      expect project-edit "$([ "$e" = 1920,1080 ] && echo PASS || echo FAIL)" "edit.mp4 ${e:-missing}"
    fi
  fi
fi

if want project-plate; then
  P=$(project project-plate <<YAML
type: video
aspect_ratio: "16:9"
defaults: {shot: {with: $H3GEN}}
entities:
  - {name: pattern, kind: environment, description: "a synthetic broadcast test pattern", image: plate.png}
shots:
  - {name: follow, prompt: "<pattern>: colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears.",
     duration_s: 5, plate: {video: plate.mp4}, render: {options: {seed: $((R + 301))}}}
YAML
)
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=5 -c:v libx264 -pix_fmt yuv420p "$P/plate.mp4"
  nice -n 19 ffmpeg -v error -y -i "$P/plate.mp4" -frames:v 1 "$P/plate.png"
  render project-plate "$P" follow
  f=$(jq -r "$PRIMARY" <<< "$(approved "$P" follow)")
  r=$([ -n "$f" ] && "${FR[@]}" guide "$P/plate.mp4" "$P/$f" 2>&1 | tail -1)
  expect project-plate "$(jq -r 'if .within_1 >= 0.8 then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" \
    "${r:-no version: $(tail -c 300 project-plate.log)}"
fi

if want project-voice; then
  P=$(project project-voice <<YAML
type: video
aspect_ratio: "16:9"
defaults: {shot: {with: $H3GEN}, clean: {with: $CLEAN}}
entities:
  - {name: host, kind: character, description: "$CHAR", image: hero.png, voice: voice.wav}
  - {name: yard, kind: environment, description: "$SCENE", image: place.png}
shots:
  - {name: hello, prompt: "<host> stands in <yard>, looks into the camera and says \"Welcome to the yard.\"", duration_s: 5,
     render: {options: {seed: $((R + 302))}}}
YAML
)
  # The voice: 4 s of the split shot's soundtrack (its "Done."), else of the gate's H3 clip, else a tone.
  src=$(ls "$OUT"/project-split/renders/mural/*.mp4 2>/dev/null | head -1); src=${src:-$CLIP}
  if [ -n "$src" ]; then nice -n 19 ffmpeg -v error -y -ss 5 -i "$src" -t 4 -vn -ac 1 -ar 24000 "$P/voice.wav"
  else nice -n 19 ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=4 -ac 1 -ar 24000 "$P/voice.wav"; fi
  render project-voice "$P" hello
  v=$(approved "$P" hello); f=$(jq -r "$PRIMARY" <<< "$v")
  # Sent: a piece's logged references name the character's voice (H3 attaches it as "<name>'s voice").
  sent=$([ -n "$f" ] && "$CLI" run show "$(jq -r '.runs[-1]' <<< "$v")" --once 2>/dev/null | grep -o "references=\"[^\"]*host's voice[^\"]*\"" | head -1)
  a=$([ -n "$f" ] && ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$P/$f")
  expect project-voice "$([ -n "$sent" ] && [ -n "$a" ] && echo PASS || echo FAIL)" \
    "voice ${sent:-not sent}, sound ${a:-none} s${f:+ (run $(jq -r '.runs[-1]' <<< "$v"))}"
fi

if want project-stills; then
  P=$(project project-stills <<YAML
type: video
aspect_ratio: "16:9"
defaults: {entity: {with: $STILL}, setup: {with: $STILL}, clean: {with: $CLEAN}}
entities:
  - {name: cook, kind: character, description: "$CHAR", render: {options: {seed: $((R + 303))}}}
  - name: kitchen
    kind: environment
    description: "$SCENE"
    render: {options: {seed: $((R + 304))}}
    setups: [{name: a, camera: "wide from the door: the nearest objects large on the left, the window small on the right"}]
YAML
)
  render project-stills "$P" all
  for n in kitchen kitchen.a; do
    f=$(jq -r "$PRIMARY" <<< "$(approved "$P" $n)")
    expect "project-stills-$n" "$([ -n "$f" ] && near "$(aspect "$P/$f")" 1.7778 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 16:9"
  done
  expect project-stills-clean "$(lockq "$P" "'PASS' if any(v.get('cleaned') for v in lock['nodes']['cook']['versions']) else 'FAIL'" || echo FAIL)" \
    "cook's picture cleaned before use"
  sed -i 's/aspect_ratio: "16:9"/aspect_ratio: "9:16"/' "$P/project.yaml"
  render project-stills-916 "$P" kitchen
  f=$(lockq "$P" "lock['nodes']['kitchen']['versions'][-1]['assets']['image']['path']")
  expect project-stills-916 "$([ -n "$f" ] && near "$(aspect "$P/$f")" 0.5625 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 9:16"
fi

if want project-local; then
  P=$(project project-local <<YAML
type: video
aspect_ratio: "16:9"
entities:
  - {name: lot, kind: environment, description: "$SCENE", render: {with: ./still.py/draw}}
YAML
)
  cp "$HERE/suites/project/still.py" "$P/still.py"
  render project-local "$P" lot
  v=$(approved "$P" lot); f=$(jq -r "$PRIMARY" <<< "$v")
  expect project-local "$([ -n "$f" ] && [[ $(jq -r .made_by.with <<< "$v") == local/still/draw@* ]] && [ "$(near "$(aspect "$P/$f")" 1.7778)" = PASS ] && echo PASS || echo FAIL)" \
    "made by $(jq -r '.made_by.with // "nothing"' <<< "$v")${f:+, aspect $(aspect "$P/$f")}: $(tail -c 200 project-local.log)"
fi

if want project-refusals; then
  P=$(project project-refusals <<YAML
type: video
aspect_ratio: "16:9"
entities:
  - {name: yard, kind: environment, description: "$SCENE", image: place.png}
shots:
  - {name: plated, prompt: "<yard>.", duration_s: 5, plate: {video: plate.mp4}, render: {with: $SDXL}}
  - {name: square, prompt: "<yard>.", duration_s: 5, render: {with: $SDXL, options: {aspect_ratio: "1:1"}}}
YAML
)
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=5 -c:v libx264 -pix_fmt yuv420p "$P/plate.mp4"
  for c in "plated:takes no plate" "square:set by the project"; do
    n=${c%%:*} why=${c#*:}
    if render "project-refusal-$n" "$P" "$n" || ! grep -q "$why" "project-refusal-$n.log"; then
      expect "project-refusal-$n" FAIL "not refused ($why): $(tail -c 300 "project-refusal-$n.log")"
    else
      runs=$(lockq "$P" "sum(len(n.get('versions', [])) for n in lock['nodes'].values()) + len(lock.get('pending', {}))")
      expect "project-refusal-$n" "$([ "${runs:-0}" = 0 ] && echo PASS || echo FAIL)" "$(grep -m1 "$why" "project-refusal-$n.log"); ${runs:-0} runs"
    fi
  done
fi
