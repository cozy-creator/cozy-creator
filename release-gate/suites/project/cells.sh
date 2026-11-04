# Suite `project` (xs-151, cl-758): `cozy project render` on fixture projects, format 2, the walk in this computer's Calcifer (the
# candidate cozy), the runs on $M. Each cell is a project folder under $OUT, judged from its project.lock and files:
# - project-split: a 10 s shot whose later beat reads a world state an earlier beat writes: one run, one take, the written frame
#   kept, no seam of ours; project-upscale: its upscale from those latents, at 1080p and the shot's aspect, and edit.mp4 at the
#   1920x1080 canvas.
# - project-in: a new take that keeps take 1 up to `--in` and renders the rest from edited text: one run, the kept frames as
#   they were, the join smooth, the record exact (xs-150).
# - project-plate: a shot with `plate:` follows it frame by frame (guide-equal's bar).
# - project-voice: a character with `voice:` speaks; the voice goes with the request and the video has sound.
# - project-stills: a place and its setup at the canvas aspect, at 16:9 and again at 9:16; a character's picture at 16:9, and its
#   clean, an asset of its own, that returns what it reads at 5% fully transparent or more (BiRefNet composites on white by design).
# - project-local: an asset `with: ./still.py`, local code made ready by Calcifer.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# A checkout's ./pkg names the gate's folder; a project reads `with:` from its own. `with:` names a package, never a function.
abs() { [[ $1 == ./* ]] && echo "$OUT/${1#./}" || echo "$1"; }
package() { local at; at=$(abs "$(pkg "$1" x)"); case $at in *@*) echo "${at%/x@*}@${at##*@}" ;; *) echo "${at%/x}" ;; esac; }
H3=$(package minimax-h3) STILL=$(package qwen-image-2) CLEAN=$(package birefnet)
want() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* || $ONLY == *",project,"* ]]; }
# project <cell>: a fresh folder for it, its project.yaml from stdin, the gate's pictures beside it.
project() {
  local P=$OUT/$1; rm -rf "$P"; mkdir -p "$P"; cat > "$P/project.yaml"
  nice -n 19 ffmpeg -v error -y -i "${K:-$REF}" "$P/hero.png"; nice -n 19 ffmpeg -v error -y -i "$REF" "$P/place.png"
  echo "$P"
}
# imported <folder> <file>…: each file `cozy project import`ed, and where project.yaml writes it, its hash prefix.
imported() {
  local P=$1 f h; shift
  for f in "$@"; do
    h=$("$CLI" project -p "$P" import "$P/$f" | tail -1 | sed 's/^[a-z]*: //')
    sed -i "s#: $f\b#: $h#" "$P/project.yaml"
  done
}
render() { local c=$1 P=$2; shift 2; "$CLI" project -p "$P" render "$@" --on "$M" > "$c.log" 2>&1; }
# lockq <folder> <python expression of lock>
lockq() { python3 -c "import json, sys; lock = json.load(open(sys.argv[1] + '/project.lock')); print($2)" "$1" 2>/dev/null; }
# approved <folder> <asset>: the record of its newest version, as JSON.
approved() { lockq "$1" "json.dumps((lock['assets'].get('$2', {}).get('versions') or [{}])[-1])"; }
# file <folder> <version record's sha256>: its path.
file_of() { [ -n "$2" ] && lockq "$1" "lock['files']['$2']['path']"; }
# made <folder> <asset>: the path of the file people mean by its newest version.
made() { file_of "$1" "$(approved "$1" "$2" | jq -r '.files.video // .files.image // empty')"; }
aspect() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1" | awk -F, '{ printf "%.4f", $1 / $2 }'; }
near() { awk -v a="$1" -v b="$2" 'BEGIN { d = a / b - 1; print (d < 0 ? -d : d) <= 0.03 ? "PASS" : "FAIL" }'; }
# transparent_share <picture>: its share of fully transparent pixels, as Calcifer's clean rule reads it.
transparent_share() { ffmpeg -v error -i "$1" -frames:v 1 -vf format=rgba,alphaextract -f rawvideo -pix_fmt gray - | python3 -c 'import sys; b = sys.stdin.buffer.read(); print(round(b.count(0) / len(b), 4))'; }
SEAMBAR='if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end'

if want project-split || want project-upscale; then
  P=$(project project-split <<YAML
version: 2
defaults:
  all:
    style: naturalistic photo, warm late-afternoon light, handheld camera
    aspect_ratio: "16:9"
  shot:
    with: $H3
  upscale:
    with: $H3
  edit:
    with: cozy
assets:
  hero:
    image: hero.png
    description: "$CHAR"
  yard:
    image: place.png
    description: "$SCENE"
  wall:
    image: place.png
    description: a stepladder against the wall
  mural:
    kind: shot
    options: {seed: $((R + 300))}
    summary: |
      <hero> paints a sign on the long wall of <yard>.
    beats:
      - duration: 6
        description: |
          <hero> rolls bright orange paint across the wall in big letters that read OPEN.
        writes:
          - scene: wall.1
            at: 5
      - duration: 4
        description: |
          <hero> steps back from <wall.1>, where the orange letters read OPEN, and says "Done."
  mural upscale:
    kind: upscale
    video: <mural>
    resolution: 1080p
  edit:
    kind: edit
    resolution: 1080p
    pieces:
      - <mural upscale>
YAML
)
  imported "$P" hero.png place.png
  render project-split "$P" mural || expect project-split FAIL "render mural: $(tail -c 300 project-split.log)"
  v=$(approved "$P" mural)
  f=$(made "$P" mural)
  if [ -z "$f" ]; then expect project-split FAIL "no version of mural: $(tail -c 300 project-split.log)"
  else
    expect project-split "$(jq -r 'if (.runs | length) == 1 and .files.latents != null and .writes["wall.1"].image != null then "PASS" else "FAIL" end' <<< "$v")" \
      "runs $(jq -c .runs <<< "$v"), latents $(jq -r '.files.latents // "none"' <<< "$v"), wall.1 $(jq -r '.writes["wall.1"].image // "none"' <<< "$v"), made by $(jq -r '.made_by.with' <<< "$v")"
    expect project-split-text "$(jq -r 'if ([.warnings[]? | select(test("past the end"))] | length) == 0 then "PASS" else "FAIL" end' <<< "$v")" "warnings: $(jq -c '.warnings // []' <<< "$v")"
  fi
  if want project-upscale && [ -n "$f" ]; then
    render project-upscale "$P" "mural upscale" || expect project-upscale FAIL "render mural upscale: $(tail -c 300 project-upscale.log)"
    u=$(made "$P" "mural upscale")
    if [ -z "$u" ]; then expect project-upscale FAIL "no upscale: $(tail -c 300 project-upscale.log)"
    else
      h=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$P/$u")
      expect project-upscale "$([ "$h" = 1080 ] && [ "$(near "$(aspect "$P/$u")" "$(aspect "$P/$f")")" = PASS ] && echo PASS || echo FAIL)" \
        "upscale $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$u"), shot $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$f")"
      lockq "$P" "lock['assets']['mural upscale']['versions'][-1]['invocations'][-1]['input'].get('latents', '')" | grep -q sha256 \
        || expect project-upscale-latents FAIL "the upscale was not sent the shot's latents"
      e=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/edit.mp4" 2>/dev/null)
      expect project-edit "$([ "$e" = 1920,1080 ] && echo PASS || echo FAIL)" "edit.mp4 ${e:-missing}"
    fi
  fi
fi

if want project-in; then
  P=$(project project-in <<YAML
version: 2
defaults:
  all:
    aspect_ratio: "16:9"
  shot:
    with: $H3
assets:
  yard:
    image: place.png
    description: "$SCENE"
  look:
    kind: shot
    options: {seed: $((R + 305))}
    summary: |
      A slow look around <yard>.
    soundscape: A quiet street, distant traffic and birds.
    beats:
      - duration: 6
        description: |
          The camera drifts slowly right along <yard>.
      - duration: 5
        description: |
          The camera keeps drifting right, past the far wall.
YAML
)
  imported "$P" place.png
  render project-in-first "$P" look || expect project-in FAIL "first take: $(tail -c 300 project-in-first.log)"
  old=$(made "$P" look)
  sed -i 's/past the far wall/and tilts up toward the sky/' "$P/project.yaml"
  render project-in "$P" look --take 1 --in 6 || expect project-in FAIL "--in: $(tail -c 300 project-in.log)"
  v=$(approved "$P" look); f=$(made "$P" look); until_s=$(jq -r '.kept.until_s // empty' <<< "$v")
  expect project-in "$(jq -r 'if .number == 2 and .kept.take == 1 and (.kept.until_s - 6 | fabs) <= 0.36 and (.runs | length) == 1 and .files.latents != null then "PASS" else "FAIL" end' <<< "$v")" \
    "take $(jq -r .number <<< "$v"), kept $(jq -c .kept <<< "$v"), runs $(jq -c .runs <<< "$v"): $(grep -o 'look: take 2 is .*' project-in.log | head -1)"
  if [ -n "$until_s" ] && [ -n "$f" ]; then
    r=$("${FR[@]}" seams "$P/$f" --at "$until_s" 2>&1 | tail -1)
    expect project-in-seam "$(jq -r "$SEAMBAR" <<< "$r" 2>/dev/null || echo FAIL)" "join at $until_s s: $r"
    # The same latents, decoded again: the kept part up to its last second is the same picture.
    s=$(nice -n 19 ffmpeg -nostdin -v info -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$old" -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$f" -lavfi ssim -f null - 2>&1 | grep -oE 'All:[0-9.]+' | tail -1 | cut -d: -f2)
    expect project-in-kept "$(awk -v s="${s:-0}" 'BEGIN { print s >= 0.95 ? "PASS" : "FAIL" }')" "SSIM of the kept part to take 1: ${s:-none}"
  fi
fi

if want project-plate; then
  P=$(project project-plate <<YAML
version: 2
defaults:
  all:
    aspect_ratio: "16:9"
  shot:
    with: $H3
assets:
  pattern:
    image: plate.png
    description: a synthetic broadcast test pattern
  take one:
    video: plate.mp4
  follow:
    kind: shot
    options: {seed: $((R + 301))}
    summary: "<pattern>: colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears."
    beats:
      - duration: 5
    plate:
      video: <take one>
YAML
)
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=5 -c:v libx264 -pix_fmt yuv420p "$P/plate.mp4"
  nice -n 19 ffmpeg -v error -y -i "$P/plate.mp4" -frames:v 1 "$P/plate.png"
  imported "$P" plate.png plate.mp4
  render project-plate "$P" follow
  f=$(made "$P" follow)
  r=$([ -n "$f" ] && "${FR[@]}" guide "$P/plate.mp4" "$P/$f" 2>&1 | tail -1)
  expect project-plate "$(jq -r 'if .within_1 >= 0.8 then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" \
    "${r:-no version: $(tail -c 300 project-plate.log)}"
fi

if want project-voice; then
  P=$(project project-voice <<YAML
version: 2
defaults:
  all:
    aspect_ratio: "16:9"
  shot:
    with: $H3
assets:
  speech:
    audio: voice.wav
  host:
    image: hero.png
    description: "$CHAR"
    voice: <speech>
  yard:
    image: place.png
    description: "$SCENE"
  hello:
    kind: shot
    options: {seed: $((R + 302))}
    summary: <host> stands in <yard>, looks into the camera and says "Welcome to the yard."
    beats:
      - duration: 5
YAML
)
  # The voice: 4 s of the split shot's soundtrack (its "Done."), else of the gate's H3 clip, else a tone.
  src=$(made "$OUT/project-split" mural); src=${src:+$OUT/project-split/$src}; src=${src:-$CLIP}
  if [ -n "$src" ]; then nice -n 19 ffmpeg -v error -y -ss 5 -i "$src" -t 4 -vn -ac 1 -ar 24000 "$P/voice.wav"
  else nice -n 19 ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=4 -ac 1 -ar 24000 "$P/voice.wav"; fi
  imported "$P" hero.png place.png voice.wav
  render project-voice "$P" hello
  v=$(approved "$P" hello); f=$(made "$P" hello)
  # Sent: a piece's logged references name the character's voice (H3 attaches it as "<name>'s voice").
  sent=$([ -n "$f" ] && "$CLI" run show "$(jq -r '.runs[-1]' <<< "$v")" --once 2>/dev/null | grep -o "references=\"[^\"]*host's voice[^\"]*\"" | head -1)
  a=$([ -n "$f" ] && ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$P/$f")
  expect project-voice "$([ -n "$sent" ] && [ -n "$a" ] && echo PASS || echo FAIL)" \
    "voice ${sent:-not sent}, sound ${a:-none} s${f:+ (run $(jq -r '.runs[-1]' <<< "$v"))}"
fi

if want project-stills; then
  P=$(project project-stills <<YAML
version: 2
defaults:
  all:
    aspect_ratio: "16:9"
  character:
    with: $STILL
  environment:
    with: $STILL
  setup:
    with: $STILL
assets:
  cook:
    kind: character
    description: "$CHAR"
    options: {seed: $((R + 303))}
  cook clean:
    with: $CLEAN
    kind: clean
    image: <cook>
  kitchen:
    kind: environment
    description: "$SCENE"
    options: {seed: $((R + 304))}
  kitchen door:
    kind: setup
    camera: "<kitchen> wide from the door: the nearest objects large on the left, the window small on the right"
YAML
)
  render project-stills "$P" all
  for n in kitchen "kitchen door" cook; do
    f=$(made "$P" "$n")
    expect "project-stills-${n// /-}" "$([ -n "$f" ] && near "$(aspect "$P/$f")" 1.7778 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 16:9"
  done
  drawn=$(made "$P" cook); clean=$(made "$P" "cook clean")
  share=$([ -n "$drawn" ] && transparent_share "$P/$drawn")
  # Drawn transparent, the clean is the same file with no run; drawn opaque, it is a new one.
  same=$([ "$drawn" = "$clean" ] && echo True || echo False)
  expect project-stills-transparent "$(awk -v s="${share:-x}" -v c="$same" 'BEGIN { print (s != "x" && (s >= 0.05 ? "True" : "False") == c) ? "PASS" : "FAIL" }')" "cook's picture ${share:-unread} fully transparent, its clean the same file: $same"
  sed -i '0,/aspect_ratio: "16:9"/s//aspect_ratio: "9:16"/' "$P/project.yaml"
  render project-stills-916 "$P" kitchen
  f=$(made "$P" kitchen)
  expect project-stills-916 "$([ -n "$f" ] && near "$(aspect "$P/$f")" 0.5625 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 9:16"
fi

if want project-local; then
  P=$(project project-local <<YAML
version: 2
assets:
  lot:
    with: ./still.py
    kind: environment
    aspect_ratio: "16:9"
    description: "$SCENE"
YAML
)
  cp "$HERE/suites/project/still.py" "$P/still.py"
  render project-local "$P" lot
  v=$(approved "$P" lot); f=$(made "$P" lot)
  expect project-local "$([ -n "$f" ] && [[ $(jq -r .made_by.with <<< "$v") == local/still/draw* ]] && [ "$(near "$(aspect "$P/$f")" 1.7778)" = PASS ] && echo PASS || echo FAIL)" \
    "made by $(jq -r '.made_by.with // "nothing"' <<< "$v")${f:+, aspect $(aspect "$P/$f")}: $(tail -c 200 project-local.log)"
fi
