# Suite `project` (xs-151, cl-758): `cozy project render` on fixture projects, format 2, the walk in this computer's Calcifer (the
# candidate cozy), the runs on $M. Each cell is a project folder under $OUT, judged from its project.lock and files:
# - project-split: a 10 s shot of two beats: one run, one take, its latents kept, no seam of ours; project-upscale: its upscale from those latents, at 1080p and the shot's aspect, and edit.mp4 at the
#   1920x1080 canvas.
# - project-segments: a shot two segments make, carrying 22 and 56 frames: both segments ran, no cut at the join.
# - project-in: a new take that keeps take 1 up to `--in` and renders the rest from edited text: one run, the kept frames as
#   they were, the join smooth, the record exact (xs-150).
# - project-plate: a shot with `plate:` follows it frame by frame (guide-equal's bar).
# - project-voice: an entity with a `voice:` link speaks; the voice goes with the request and the video has sound.
# - project-stills: a place and its setup at the canvas aspect, at 16:9 and again at 9:16; a character's picture at 16:9.
# - project-local: an asset `tool: ./still.py/draw`, local code made ready by Calcifer.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# A checkout's ./pkg names the gate's folder; a project reads `tool:` from its own. `tool:` names a package, never a function.
abs() { [[ $1 == ./* ]] && echo "$OUT/${1#./}" || echo "$1"; }
package() { local at; at=$(abs "$(pkg "$1" x)"); case $at in *@*) echo "${at%/x@*}@${at##*@}" ;; *) echo "${at%/x}" ;; esac; }
H3=$(package minimax-h3) STILL=$(package qwen-image-2)
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
    h=$("$CLI" --json project -p "$P" import "$P/$f" | jq -r '.sha256 | sub("sha256:"; "") | .[:12]')
    sed -i "s#: $f\b#: $h#" "$P/project.yaml"
  done
}
render() { local c=$1 P=$2; shift 2; "$CLI" project -p "$P" render "$@" --on "$M" > "$c.log" 2>&1; }
# approved <folder> <asset>: the record of its newest version, as JSON.
approved() { "$CLI" --json --full project -p "$1" status "$2" | jq -c '(.versions[-1] // {}) | . + {files: (.files | map_values(.sha256)), run: (.runs[-1] // null)}'; }
# file <folder> <version record's sha256>: its path.
file_of() { [ -n "$2" ] && (cd "$1" && ls "files/${2#sha256:}"* | head -1); }
# made <folder> <asset>: the path of the file people mean by its newest version.
made() { file_of "$1" "$(approved "$1" "$2" | jq -r '.files.video // .files.image // empty')"; }
aspect() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1" | awk -F, '{ printf "%.4f", $1 / $2 }'; }
near() { awk -v a="$1" -v b="$2" 'BEGIN { d = a / b - 1; print (d < 0 ? -d : d) <= 0.03 ? "PASS" : "FAIL" }'; }
SEAMBAR='if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end'

if want project-split || want project-upscale; then
  P=$(project project-split <<YAML
version: 3
defaults:
  video:
    aspect_ratio: "16:9"
assets:
  look_style:
    text: Naturalistic photo, warm late-afternoon light, handheld camera.
  mural:
    tool: $H3/generate
    seed: $((R + 300))
    summary: |
      <look_style> <hero_sheet> paints a sign on the long wall of <yard>.
    beats:
      - duration: 6
        description: |
          <hero_sheet> rolls bright orange paint across the wall in big letters that read OPEN.
      - duration: 4
        description: |
          <hero_sheet> steps back from the wall of <yard>, where the orange letters read OPEN, and says "Done."
  hero_art:
    approved: hero.png
  hero_sheet:
    tool: ./still.py/keep
    image: <hero_art>
    description: "$CHAR"
    asset_type: character
  yard:
    approved: place.png
    description: "$SCENE"
  mural_upscale:
    tool: $H3/upscale
    video: <mural>
    resolution: 1080p
  edit:
    tool: cozy/edit
    aspect_ratio: "16:9"
    resolution: 1080p
    tracks:
      - clips:
          - <mural_upscale>
YAML
)
  imported "$P" hero.png place.png
  cp "$HERE/suites/project/still.py" "$P/still.py"
  render project-split "$P" mural || expect project-split FAIL "render mural: $(tail -c 300 project-split.log)"
  v=$(approved "$P" mural)
  f=$(made "$P" mural)
  if [ -z "$f" ]; then expect project-split FAIL "no version of mural: $(tail -c 300 project-split.log)"
  else
    expect project-split "$(jq -r 'if .run != null and .files.latents != null then "PASS" else "FAIL" end' <<< "$v")" \
      "run $(jq -c .run <<< "$v"), latents $(jq -r '.files.latents // "none"' <<< "$v")"
    expect project-split-text "$(jq -r 'if ([.warnings[]?.message | select(test("past the end"))] | length) == 0 then "PASS" else "FAIL" end' <<< "$v")" "warnings: $(jq -c '.warnings // []' <<< "$v")"
    # The gate's hero is a character whose sheet its package made, with a background: H3 says it goes into the shot
    # (se-104). A warning, no failed run. An imported picture is left alone, since it may be opaque on purpose.
    expect project-opaque "$(grep -q "picture is opaque" project-split.log && echo PASS || echo FAIL)" "$(grep -m1 opaque project-split.log | cut -c1-160)"
  fi
  if want project-upscale && [ -n "$f" ]; then
    render project-upscale "$P" mural_upscale || expect project-upscale FAIL "render mural upscale: $(tail -c 300 project-upscale.log)"
    u=$(made "$P" "mural upscale")
    if [ -z "$u" ]; then expect project-upscale FAIL "no upscale: $(tail -c 300 project-upscale.log)"
    else
      h=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$P/$u")
      expect project-upscale "$([ "$h" = 1080 ] && [ "$(near "$(aspect "$P/$u")" "$(aspect "$P/$f")")" = PASS ] && echo PASS || echo FAIL)" \
        "upscale $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$u"), shot $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$f")"
      "$CLI" --json --full run watch "$("$CLI" --json project -p "$P" status mural_upscale | jq -r '.versions[-1].runs[-1]')" --once | jq -r '.request.input.latents // ""' | grep -q sha256 \
        || expect project-upscale-latents FAIL "the upscale was not sent the shot's latents"
      e=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/edit.mp4" 2>/dev/null)
      expect project-edit "$([ "$e" = 1920,1080 ] && echo PASS || echo FAIL)" "edit.mp4 ${e:-missing}"
    fi
  fi
fi

if want project-segments; then
  # A shot two segments make, with a write at their join, as a long shot is: the later one carries the earlier's frames.
  for cf in 22 56; do
    P=$(project project-segments-$cf <<YAML
version: 3
defaults:
  video:
    aspect_ratio: "16:9"
assets:
  look_style:
    text: Naturalistic photo, warm late-afternoon light, handheld camera.
  hero_art:
    approved: hero.png
    description: "$CHAR"
    asset_type: character
  yard_art:
    approved: place.png
    description: "$SCENE"
    asset_type: environment
  walk:
    tool: $H3/generate
    seed: $((R + 330 + cf))
    context_frames: $cf
    summary: |
      <look_style> <hero_art> walks the length of <yard_art> in one continuous shot with no cuts.
    beats:
      - duration: 10
        description: |
          <hero_art> sets off across <yard_art>, the camera following alongside.
      - duration: 10
        description: <hero_art> keeps walking across <yard_art> and stops at the far side.
YAML
)
    imported "$P" hero.png place.png
    render project-segments-$cf "$P" walk || expect project-segments-$cf FAIL "render: $(tail -c 300 project-segments-$cf.log)"
    v=$(approved "$P" walk); f=$(made "$P" walk); n=$(jq -r '.run // empty' <<< "$v")
    if [ -z "$f" ] || [ -z "$n" ]; then expect project-segments-$cf FAIL "no version of walk: $(tail -c 300 project-segments-$cf.log)"; continue; fi
    labels=$("$CLI" --json --full run watch "$n" | jq -c '[.calls[]? | {label, status}]')
    given=$("$CLI" --json --full run watch "$(jq -r .run <<< "$v")" --once | jq -c '[.request.input.context_frames]')
    expect project-segments-$cf "$(jq -r 'if ([.[] | select(.status == "succeeded")] | length) == 2 and (.[1].label == "Segment 2 of 2") then "PASS" else "FAIL" end' <<< "$labels" 2>/dev/null || echo FAIL)" "calls $labels, context_frames $given, want $cf"
    r=$("${FR[@]}" seams "$P/$f" --at "$(awk 'BEGIN { for (i = -4; i <= 4; i++) printf "%s%g", (i > -4 ? "," : ""), 10 + i / 8 }')" 2>&1 | tail -1)
    expect project-segments-$cf-seam "$(jq -r 'if ([.boundaries[].ratio] | max) < 3 then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "no cut within half a second of the join at 10 s: $r"
  done
fi

if want project-in; then
  P=$(project project-in <<YAML
version: 3
defaults:
  video:
    aspect_ratio: "16:9"
assets:
  look:
    tool: $H3/generate
    seed: $((R + 305))
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
  yard:
    approved: place.png
    description: "$SCENE"
YAML
)
  imported "$P" place.png
  render project-in-first "$P" look || expect project-in FAIL "first take: $(tail -c 300 project-in-first.log)"
  old=$(made "$P" look)
  sed -i 's/past the far wall/and tilts up toward the sky/' "$P/project.yaml"
  render project-in "$P" look --take 1 --in 6 || expect project-in FAIL "--in: $(tail -c 300 project-in.log)"
  v=$(approved "$P" look); f=$(made "$P" look); until_s=$(jq -r '.kept.until_s // empty' <<< "$v")
  expect project-in "$(jq -r 'if .number == 2 and .kept.take == 1 and (.kept.until_s - 6 | fabs) <= 0.36 and (.run != null) and .files.latents != null then "PASS" else "FAIL" end' <<< "$v")" \
    "take $(jq -r .number <<< "$v"), kept $(jq -c .kept <<< "$v"), run $(jq -c .run <<< "$v"): $(grep -o 'look: take 2 is .*' project-in.log | head -1)"
  if [ -n "$until_s" ] && [ -n "$f" ]; then
    r=$("${FR[@]}" seams "$P/$f" --at "$until_s" 2>&1 | tail -1)
    expect project-in-seam "$(jq -r "$SEAMBAR" <<< "$r" 2>/dev/null || echo FAIL)" "join at $until_s s: $r"
    # The same latents, decoded again: the kept part up to its last second is the same picture.
    s=$(nice -n 19 ffmpeg -nostdin -v info -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$old" -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$f" -lavfi ssim -f null - 2>&1 | grep -oE 'All:[0-9.]+' | tail -1 | cut -d: -f2)
    expect project-in-kept "$(awk -v s="${s:-0}" 'BEGIN { print s >= 0.95 ? "PASS" : "FAIL" }')" "SSIM of the kept part to take 1: ${s:-none}"
  fi
  # H3 refuses: a failed run on purpose, so this suite only ever runs on a gate machine, never a person's history.
  render project-in-end "$P" look --take 2; code=$?
  expect project-in-end "$([ "$code" != 0 ] && grep -q "already reaches the shot's end" project-in-end.log && echo PASS || echo FAIL)" "exit $code: $(tail -c 200 project-in-end.log)"
fi

if want project-plate; then
  P=$(project project-plate <<YAML
version: 3
defaults:
  video:
    aspect_ratio: "16:9"
assets:
  follow:
    tool: $H3/generate
    seed: $((R + 301))
    summary: "<pattern>: colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears."
    beats:
      - duration: 5
    plate:
      video: <take_one>
  pattern:
    approved: plate.png
    description: a synthetic broadcast test pattern
  take_one:
    approved: plate.mp4
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
version: 3
defaults:
  video:
    aspect_ratio: "16:9"
assets:
  hello:
    tool: $H3/generate
    seed: $((R + 302))
    summary: <host_sheet> stands in <yard>, looks into the camera and says "Welcome to the yard."
    beats:
      - duration: 5
  host_sheet:
    approved: hero.png
    description: "$CHAR"
    asset_type: character
    voice: <host_voice>
  host_voice:
    approved: voice.wav
  yard:
    approved: place.png
    description: "$SCENE"
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
  sent=$([ -n "$f" ] && "$CLI" run watch --full "$(jq -r .run <<< "$v")" --once 2>/dev/null | grep -o "references=\"[^\"]*host's voice[^\"]*\"" | head -1)
  a=$([ -n "$f" ] && ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$P/$f")
  expect project-voice "$([ -n "$sent" ] && [ -n "$a" ] && echo PASS || echo FAIL)" \
    "voice ${sent:-not sent}, sound ${a:-none} s${f:+ (run $(jq -r .run <<< "$v"))}"
fi

if want project-stills; then
  P=$(project project-stills <<YAML
version: 3
defaults:
  image:
    aspect_ratio: "16:9"
assets:
  cook:
    tool: $STILL/generate
    instructions: "$CHAR"
    seed: $((R + 303))
  kitchen:
    tool: $STILL/generate
    instructions: "$SCENE"
    seed: $((R + 304))
  kitchen_door:
    tool: $STILL/generate
    instructions: "<kitchen> wide from the door: the nearest objects large on the left, the window small on the right"
YAML
)
  render project-stills "$P" all
  for n in kitchen "kitchen door" cook; do
    f=$(made "$P" "$n")
    expect "project-stills-${n// /-}" "$([ -n "$f" ] && near "$(aspect "$P/$f")" 1.7778 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 16:9"
  done
  sed -i '0,/aspect_ratio: "16:9"/s//aspect_ratio: "9:16"/' "$P/project.yaml"
  render project-stills-916 "$P" kitchen
  f=$(made "$P" kitchen)
  expect project-stills-916 "$([ -n "$f" ] && near "$(aspect "$P/$f")" 0.5625 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 9:16"
fi

if want project-local; then
  P=$(project project-local <<YAML
version: 3
assets:
  lot:
    tool: ./still.py/draw
    instructions: "$SCENE"
    aspect_ratio: "16:9"
YAML
)
  cp "$HERE/suites/project/still.py" "$P/still.py"
  render project-local "$P" lot
  v=$(approved "$P" lot); f=$(made "$P" lot)
  expect project-local "$([ -n "$f" ] && [ "$(jq -r '.run // ""' <<< "$v")" != "" ] && [ "$(near "$(aspect "$P/$f")" 1.7778)" = PASS ] && echo PASS || echo FAIL)" \
    "run $(jq -r '.run // "none"' <<< "$v")${f:+, aspect $(aspect "$P/$f")}: $(tail -c 200 project-local.log)"
fi
