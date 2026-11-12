# Suite `project` (xs-151, cl-758): fixture projects rendered as the page renders them (e2e/page.py: from 0.24 the folder is the
# project and no CLI verb renders one), its boxes made through the page's API, the walk in this computer's Calcifer (the candidate
# cozy), the runs on $M. Each cell is a project folder under $OUT, judged from its boxes' versions and files:
# - project-split: a 10 s shot of two beats: one run, one take, its latents kept, no seam of ours; project-upscale: its upscale from
#   those latents, at 1080p and the shot's aspect, and the edit at the 1920x1080 canvas.
# - project-segments: a shot two segments make, carrying 22 and 56 frames: both segments ran, no cut at the join.
# - project-in: a new take that picks up take 1 at a frame and renders the rest from edited text: one run, the kept frames as
#   they were, the join smooth, the record exact (xs-150).
# - project-plate: an edit of a plate follows it frame by frame (guide-equal's bar).
# - project-voice: a sound the text mentions speaks: it goes with the request as a voice, and the video has sound.
# - project-stills: a place and its setup at the canvas aspect, at 16:9 and again at 9:16; a character's picture at 16:9.
# - project-local: an asset `tool: ./still.py/draw`, local code made ready by Calcifer.
FR=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/frames.py")
# A checkout's ./pkg names the gate's folder; a project reads `tool:` from its own.
abs() { [[ $1 == ./* ]] && echo "$OUT/${1#./}" || echo "$1"; }
package() { local at; at=$(abs "$(pkg "$1" x)"); case $at in *@*) echo "${at%/x@*}@${at##*@}" ;; *) echo "${at%/x}" ;; esac; }
H3=$(package minimax-h3) STILL=$(package qwen-image-2)
want() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* || $ONLY == *",project,"* ]]; }
page() { python3 "$RUN_PAGE" "$@"; }
# project <cell>: a fresh project folder for it (the gate's own project stays $RUN_PROJECT), the gate's pictures in it.
project() {
  local P=$OUT/$1; rm -rf "$P"; mkdir -p "$P"
  (run_project "$P") > /dev/null 2>&1 || log "$1: no project at $P"
  nice -n 19 ffmpeg -v error -y -i "${K:-$REF}" "$P/hero.png"; nice -n 19 ffmpeg -v error -y -i "$REF" "$P/place.png"
  echo "$P"
}
# imported <folder> <name> <file> [description]: the file a box of that name approving it, as the page drops it.
imported() { page import "$1" "$1/$3" --name "$2" ${4:+--description "$4"} > /dev/null || log "$(basename "$1"): import $3 as $2 refused"; }
# boxes <folder>: stdin's JSON array of boxes (POST /assets bodies) made in it, in order, as the page makes them.
boxes() { page boxes "$1" > /dev/null || log "$(basename "$1"): its boxes were refused"; }
# The gate's text, its seed and its tools, for a fixture's jq.
FIXTURE=(--arg char "$CHAR" --arg scene "$SCENE" --arg h3 "$H3" --arg still "$STILL" --argjson r "$R")
# render <cell> <folder> [page.py render options] <asset>…: rendered on $M, followed until each new version lands.
render() { local c=$1 P=$2; shift 2; page render "$P" --on "$M" "$@" > "$c.log" 2>&1; }
# approved <folder> <asset>: the record of its newest version, as JSON: its files by output, the run that made it.
approved() { page asset "$1" "$2" | jq -c '(.versions[-1] // {}) | . + {run: (.runs[-1] // null)}'; }
# made <folder> <asset>: the path in the folder of the file people mean by its newest version.
made() { approved "$1" "$2" | jq -r '.files.video.path // .files.image.path // empty'; }
# request <run>: what the run was asked, whole.
request() { "$CLI" --json --full run watch "$1" --once | jq -c '.request.input'; }
aspect() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$1" | awk -F, '{ printf "%.4f", $1 / $2 }'; }
near() { awk -v a="$1" -v b="$2" 'BEGIN { d = a / b - 1; print (d < 0 ? -d : d) <= 0.03 ? "PASS" : "FAIL" }'; }
SEAMBAR='if (.boundaries | length) >= 1 and ((([0.6, 0.9 * (.inside_ssim // 1)] | min) as $bar | [.boundaries[] | .ratio <= 3 and .ssim >= $bar] | all)) then "PASS" else "FAIL" end'

if want project-split || want project-upscale; then
  P=$(project project-split)
  cp "$HERE/suites/project/still.py" "$P/still.py"
  imported "$P" hero_art hero.png; imported "$P" yard place.png "$SCENE"
  # A video tool's box 16:9, as the canvas is.
  jq -n "${FIXTURE[@]}" '[
    {name: "look_style", fields: {text: "Naturalistic photo, warm late-afternoon light, handheld camera."}},
    {name: "mural", tool: "\($h3)/generate", kind: "video", fields: {aspect_ratio: "16:9", seed: ($r + 300),
      summary: "<look_style> <hero_sheet> paints a sign on the long wall of <yard>.",
      beats: [{duration: 6, description: "<hero_sheet> rolls bright orange paint across the wall in big letters that read OPEN."},
              {duration: 4, description: "<hero_sheet> steps back from the wall of <yard>, where the orange letters read OPEN, and says \"Done.\""}]}},
    {name: "hero_sheet", tool: "./still.py/keep", kind: "image", fields: {image: "<hero_art>", description: $char}},
    {name: "mural_upscale", tool: "\($h3)/upscale", kind: "video", fields: {aspect_ratio: "16:9", video: "<mural>", to: "1080p"}},
    {name: "edit", tool: "cozy/edit", fields: {aspect_ratio: "16:9", resolution: "1080p", tracks: [{clips: ["<mural_upscale>"]}]}}
  ]' | boxes "$P"
  render project-split "$P" mural || expect project-split FAIL "render mural: $(tail -c 300 project-split.log)"
  v=$(approved "$P" mural)
  f=$(made "$P" mural)
  if [ -z "$f" ]; then expect project-split FAIL "no version of mural: $(tail -c 300 project-split.log)"
  else
    expect project-split "$(jq -r 'if .run != null and .files.latents != null then "PASS" else "FAIL" end' <<< "$v")" \
      "run $(jq -c .run <<< "$v"), latents $(jq -r '.files.latents.sha256 // "none"' <<< "$v")"
    expect project-split-text "$(jq -r 'if ([.warnings[]? | select(test("past the end"))] | length) == 0 then "PASS" else "FAIL" end' <<< "$v")" "warnings: $(jq -c '.warnings // []' <<< "$v")"
    # The gate's hero is a character whose sheet its package made, with a background: H3 says it goes into the shot
    # (se-104). A warning, no failed run. An imported picture is left alone, since it may be opaque on purpose.
    expect project-opaque "$(grep -q "picture is opaque" project-split.log && echo PASS || echo FAIL)" "$(grep -o '[^"]*opaque[^"]*' project-split.log | head -1 | cut -c1-160)"
  fi
  if want project-upscale && [ -n "$f" ]; then
    render project-upscale "$P" mural_upscale || expect project-upscale FAIL "render mural upscale: $(tail -c 300 project-upscale.log)"
    u=$(made "$P" mural_upscale)
    if [ -z "$u" ]; then expect project-upscale FAIL "no upscale: $(tail -c 300 project-upscale.log)"
    else
      h=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$P/$u")
      expect project-upscale "$([ "$h" = 1080 ] && [ "$(near "$(aspect "$P/$u")" "$(aspect "$P/$f")")" = PASS ] && echo PASS || echo FAIL)" \
        "upscale $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$u"), shot $(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$f")"
      request "$(approved "$P" mural_upscale | jq -r .run)" | jq -r '.video.latents // ""' | grep -q sha256 \
        || expect project-upscale-latents FAIL "the upscale was not sent the shot's latents"
      # The edit is cut here once what it reads has landed, on its own: asked for only if it isn't yet.
      render project-edit "$P" --missing edit || expect project-edit FAIL "render edit: $(tail -c 300 project-edit.log)"
      e=$(m=$(made "$P" edit); [ -n "$m" ] && ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/$m")
      expect project-edit "$([ "$e" = 1920,1080 ] && echo PASS || echo FAIL)" "edit ${e:-missing}"
    fi
  fi
fi

if want project-segments; then
  # A shot two segments make, with a write at their join, as a long shot is: the later one carries the earlier's frames.
  for cf in 22 56; do
    P=$(project project-segments-$cf)
    imported "$P" hero_art hero.png "$CHAR"; imported "$P" yard_art place.png "$SCENE"
    jq -n "${FIXTURE[@]}" --argjson cf "$cf" '[
      {name: "look_style", fields: {text: "Naturalistic photo, warm late-afternoon light, handheld camera."}},
      {name: "walk", tool: "\($h3)/generate", kind: "video", fields: {aspect_ratio: "16:9", seed: ($r + 330 + $cf), carried_frames: $cf,
        summary: "<look_style> <hero_art> walks the length of <yard_art> in one continuous shot with no cuts.",
        beats: [{duration: 10, description: "<hero_art> sets off across <yard_art>, the camera following alongside."},
                {duration: 10, description: "<hero_art> keeps walking across <yard_art> and stops at the far side."}]}}
    ]' | boxes "$P"
    render project-segments-$cf "$P" walk || expect project-segments-$cf FAIL "render: $(tail -c 300 project-segments-$cf.log)"
    v=$(approved "$P" walk); f=$(made "$P" walk); n=$(jq -r '.run // empty' <<< "$v")
    if [ -z "$f" ] || [ -z "$n" ]; then expect project-segments-$cf FAIL "no version of walk: $(tail -c 300 project-segments-$cf.log)"; continue; fi
    labels=$("$CLI" --json --full run watch "$n" --once | jq -c '[.calls[]? | {label, status}]')
    given=$(request "$n" | jq -c '[.carried_frames]')
    expect project-segments-$cf "$(jq -r 'if ([.[] | select(.status == "succeeded")] | length) == 2 and (.[1].label == "Segment 2 of 2") then "PASS" else "FAIL" end' <<< "$labels" 2>/dev/null || echo FAIL)" "calls $labels, carried_frames $given, want $cf"
    r=$("${FR[@]}" seams "$P/$f" --at "$(awk 'BEGIN { for (i = -4; i <= 4; i++) printf "%s%g", (i > -4 ? "," : ""), 10 + i / 8 }')" 2>&1 | tail -1)
    expect project-segments-$cf-seam "$(jq -r 'if ([.boundaries[].ratio] | max) < 3 then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" "no cut within half a second of the join at 10 s: $r"
  done
fi

if want project-in; then
  # Pick up from here: take 2 keeps take 1 up to frame 141 (H3's join grid, 17k + 5: 5.875 s) and renders the rest from
  # the edited text.
  P=$(project project-in)
  imported "$P" yard place.png "$SCENE"
  jq -n "${FIXTURE[@]}" '[
    {name: "look", tool: "\($h3)/generate", kind: "video", fields: {aspect_ratio: "16:9", seed: ($r + 305),
      summary: "A slow look around <yard>.", soundscape: "A quiet street, distant traffic and birds.",
      beats: [{duration: 6, description: "The camera drifts slowly right along <yard>."},
              {duration: 5, description: "The camera keeps drifting right, past the far wall."}]}}
  ]' | boxes "$P"
  render project-in-first "$P" look || expect project-in FAIL "first take: $(tail -c 300 project-in-first.log)"
  old=$(made "$P" look)
  page set "$P" look --fields "$(page asset "$P" look | jq -c '{beats: (.asset.fields.beats | map(.description |= sub("past the far wall"; "and tilts up toward the sky")))}')" > /dev/null \
    || log "project-in: the second beat's edit was refused"
  render project-in "$P" --pickup '<look.1>' 141 look || expect project-in FAIL "pickup: $(tail -c 300 project-in.log)"
  v=$(approved "$P" look); f=$(made "$P" look); until_s=$(jq -r '.pickup.out_frame // empty | . / 24' <<< "$v")
  expect project-in "$(jq -r 'if .number == 2 and .pickup.out_frame == 141 and (.pickup.take | test("look.1")) and (.run != null) and .files.latents != null then "PASS" else "FAIL" end' <<< "$v")" \
    "take $(jq -r .number <<< "$v"), pickup $(jq -c .pickup <<< "$v"), run $(jq -c .run <<< "$v"), warnings $(jq -c '.warnings // []' <<< "$v" | cut -c1-200)"
  if [ -n "$until_s" ] && [ -n "$f" ] && [ -n "$old" ]; then
    r=$("${FR[@]}" seams "$P/$f" --at "$until_s" 2>&1 | tail -1)
    expect project-in-seam "$(jq -r "$SEAMBAR" <<< "$r" 2>/dev/null || echo FAIL)" "join at $until_s s: $r"
    # The same latents, decoded again: the kept part up to its last second is the same picture.
    s=$(nice -n 19 ffmpeg -nostdin -v info -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$old" -t "$(awk -v t="$until_s" 'BEGIN { print t - 1 }')" -i "$P/$f" -lavfi ssim -f null - 2>&1 | grep -oE 'All:[0-9.]+' | tail -1 | cut -d: -f2)
    expect project-in-kept "$(awk -v s="${s:-0}" 'BEGIN { print s >= 0.95 ? "PASS" : "FAIL" }')" "SSIM of the kept part to take 1: ${s:-none}"
  fi
  # H3 refuses a pickup past the beats' end: a failed run on purpose, so this suite only ever runs on a gate machine, never a
  # person's history.
  render project-in-end "$P" --pickup '<look.2>' 9999 look; code=$?
  expect project-in-end "$([ "$code" != 0 ] && grep -q "so nothing is made" project-in-end.log && echo PASS || echo FAIL)" "exit $code: $(tail -c 200 project-in-end.log)"
fi

if want project-plate; then
  # A plate is an edit's (minimax-h3 0.10): VFX edit, the general prompt-directed one, follows the plate frame by frame.
  P=$(project project-plate)
  nice -n 19 ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=5 -c:v libx264 -pix_fmt yuv420p "$P/plate.mp4"
  nice -n 19 ffmpeg -v error -y -i "$P/plate.mp4" -frames:v 1 "$P/plate.png"
  imported "$P" pattern plate.png "a synthetic broadcast test pattern"; imported "$P" take_one plate.mp4
  jq -n "${FIXTURE[@]}" '[
    {name: "follow", tool: "\($h3)/edit", kind: "video", fields: {seed: ($r + 301), edit: "vfx_edit", plate: "<take_one>",
      beats: [{duration: 5, description: "<pattern>: colour bars, a moving gradient sweep and a running timecode on a dark field; no person appears."}]}}
  ]' | boxes "$P"
  render project-plate "$P" follow
  f=$(made "$P" follow)
  r=$([ -n "$f" ] && "${FR[@]}" guide "$P/plate.mp4" "$P/$f" 2>&1 | tail -1)
  expect project-plate "$(jq -r 'if .within_1 >= 0.8 then "PASS" else "FAIL" end' <<< "$r" 2>/dev/null || echo FAIL)" \
    "${r:-no version: $(tail -c 300 project-plate.log)}"
fi

if want project-voice; then
  # A sound is a voice reference to H3, its speaker the one the text names with it (minimax-h3 shot.py h3_reference).
  P=$(project project-voice)
  # The voice: 4 s of the split shot's soundtrack (its "Done."), else of the gate's H3 clip, else a tone.
  src=$(made "$OUT/project-split" mural 2>/dev/null); src=${src:+$OUT/project-split/$src}; src=${src:-$CLIP}
  if [ -n "$src" ]; then nice -n 19 ffmpeg -v error -y -ss 5 -i "$src" -t 4 -vn -ac 1 -ar 24000 "$P/voice.wav"
  else nice -n 19 ffmpeg -v error -y -f lavfi -i sine=frequency=220:duration=4 -ac 1 -ar 24000 "$P/voice.wav"; fi
  imported "$P" host_sheet hero.png "$CHAR"; imported "$P" host_voice voice.wav; imported "$P" yard place.png "$SCENE"
  jq -n "${FIXTURE[@]}" '[
    {name: "hello", tool: "\($h3)/generate", kind: "video", fields: {aspect_ratio: "16:9", seed: ($r + 302),
      summary: "<host_sheet> stands in <yard>, looks into the camera and says \"Welcome to the yard.\" in the voice of <host_voice>.",
      beats: [{duration: 5}]}}
  ]' | boxes "$P"
  render project-voice "$P" hello
  v=$(approved "$P" hello); f=$(made "$P" hello)
  # Sent: the request carries host_voice's sound as one of the text's references.
  sent=$([ -n "$f" ] && request "$(jq -r .run <<< "$v")" | jq -c '[.. | objects | select(.key? == "host_voice") | {key, type}][0] // empty')
  a=$([ -n "$f" ] && ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$P/$f")
  expect project-voice "$([ -n "$sent" ] && [ -n "$a" ] && echo PASS || echo FAIL)" \
    "voice ${sent:-not sent}, sound ${a:-none} s${f:+ (run $(jq -r .run <<< "$v"))}"
fi

if want project-stills; then
  P=$(project project-stills)
  # A picture tool's box 16:9, as the canvas is.
  jq -n "${FIXTURE[@]}" '[
    {name: "cook", tool: "\($still)/image", kind: "image", fields: {aspect_ratio: "16:9", instructions: $char, seed: ($r + 303)}},
    {name: "kitchen", tool: "\($still)/image", kind: "image", fields: {aspect_ratio: "16:9", instructions: $scene, seed: ($r + 304)}},
    {name: "kitchen_door", tool: "\($still)/image", kind: "image", fields: {aspect_ratio: "16:9",
      instructions: "<kitchen> wide from the door: the nearest objects large on the left, the window small on the right"}}
  ]' | boxes "$P"
  render project-stills "$P" --wait cook,kitchen,kitchen_door all
  for n in kitchen kitchen_door cook; do
    f=$(made "$P" "$n")
    expect "project-stills-${n//_/-}" "$([ -n "$f" ] && near "$(aspect "$P/$f")" 1.7778 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 16:9"
  done
  for n in cook kitchen kitchen_door; do page set "$P" "$n" --fields '{"aspect_ratio": "9:16"}' > /dev/null || log "project-stills: $n's 9:16 refused"; done
  render project-stills-916 "$P" kitchen
  f=$(made "$P" kitchen)
  expect project-stills-916 "$([ -n "$f" ] && near "$(aspect "$P/$f")" 0.5625 || echo FAIL)" "${f:+aspect $(aspect "$P/$f")} want 9:16"
fi

if want project-local; then
  P=$(project project-local)
  cp "$HERE/suites/project/still.py" "$P/still.py"
  jq -n "${FIXTURE[@]}" '[{name: "lot", tool: "./still.py/draw", kind: "image", fields: {prompt: $scene, aspect_ratio: "16:9"}}]' | boxes "$P"
  render project-local "$P" lot
  v=$(approved "$P" lot); f=$(made "$P" lot)
  expect project-local "$([ -n "$f" ] && [ "$(jq -r '.run // ""' <<< "$v")" != "" ] && [ "$(near "$(aspect "$P/$f")" 1.7778)" = PASS ] && echo PASS || echo FAIL)" \
    "run $(jq -r '.run // "none"' <<< "$v")${f:+, aspect $(aspect "$P/$f")}: $(tail -c 200 project-local.log)"
fi
