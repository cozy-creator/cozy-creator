# join-pairs.sh, a --cells hook, proof only (cl-1210; `-f cells=release-gate/join-pairs.sh -f packages_ref=<proof/cl-1210-sound-join>`):
# hard cases for the owner to judge by ear and eye (Paul, 11-18: "Just run a few corner-case edge tests that are hard, send me the
# video"). Each shot is made twice on one seed on this pod: `-today`, the segments' sound butted as the released code does
# (the probe's `probe_butt`), and `-new`, the sound join (design/incremental-video.md "Sound joins"). Nothing here judges a take.
# - music: a steady beat and a melody in ten 5 s segments, nine joins (the probe's `probe_segment_s`: the plan as it is fits
#   three 5 s beats in a segment).
# - tone: one held organ chord, where a discontinuity is easiest to hear.
# - speech: a man speaking to the camera across two joins, his mouth in view.
# - claps: sharp hits with a visible cause.
# The package opens the first beat with `[Shot 1]` itself (`shot.written`): a beat that writes it too sends it twice.
ONLY=  # the suites' --only chose their cells, not these
jp_beats() { jq -cn '[$ARGS.positional | _nwise(2) | {duration: (.[0] | tonumber), description: .[1]}]' --args "$@"; }
JP_MUSIC=(summary="A vinyl record turns on a turntable in warm lamplight, seen from above in one fixed shot."
  music="Instrumental, no vocals: a steady kick drum on every beat at 120 beats per minute under a simple, clear piano melody in C major. The tempo, the key and the instruments stay the same throughout."
  turbo:=true probe_segment_s:=5 seed:=121001
  "beats:=$(jp_beats 5 "The record turns at a constant speed under the needle. The camera does not move." \
    5 "The record keeps turning. Nothing else changes." 5 "The record keeps turning. Nothing else changes." \
    5 "The record keeps turning. Nothing else changes." 5 "The record keeps turning. Nothing else changes." \
    5 "The record keeps turning. Nothing else changes." 5 "The record keeps turning. Nothing else changes." \
    5 "The record keeps turning. Nothing else changes." 5 "The record keeps turning. Nothing else changes." \
    5 "The record keeps turning. Nothing else changes.")")
JP_TONE=(summary="An organist's hands on the keys of a church pipe organ, in close-up, in one fixed shot."
  soundscape="One organ chord, held without a break from start to end at the same loudness: no melody, no change of note, no pause."
  turbo:=true seed:=121002
  "beats:=$(jp_beats 8 "The organist presses one chord and holds it down. The hands stay still on the keys." \
    8 "The hands hold the same chord. It keeps sounding, unbroken." 8 "The same chord still sounds. The hands have not moved.")")
JP_SPEECH=(summary="An old fisherman on a wooden pier at dusk talks to the camera in a close-up, his face and mouth clearly visible."
  soundscape="His voice, close and clear; water lapping under the pier." turbo:=true seed:=121003
  "beats:=$(jp_beats 8 "The fisherman looks into the camera and says: \"The sea was calm this morning, but the wind is turning now, you can feel it on your face.\"" \
    8 "He keeps looking into the camera and goes on without a pause: \"See those clouds out there? We will have rain by dark, and every boat is coming in early.\"" \
    8 "Still facing the camera, he keeps talking: \"My father fished these waters for forty years, and he always said the sea tells you everything if you listen.\"")")
JP_CLAPS=(summary="A woman stands facing the camera in a quiet, bare room and claps her hands in a steady beat, seen from the waist up in one fixed shot."
  soundscape="Sharp, dry hand claps, evenly spaced, two each second, in a quiet room. No music, no voice." turbo:=true seed:=121004
  "beats:=$(jp_beats 8 "She claps her hands at a steady, even pace, two claps each second. Her hands are clearly visible." \
    8 "She keeps clapping at exactly the same pace." 8 "She goes on clapping at the same steady pace, looking at the camera.")")

if [ -z "$PKGS" ]; then
  expect join-pairs FAIL "needs packages_ref, the probe branch"
  return 0
fi
JP=()
# jp <name> <inputs...>: the shot butted as today, then joined, and each take exported while the pod is here.
jp() {
  local name=$1 arm; shift
  for arm in today new; do
    if [ "$arm" = today ]; then cell "jp-$name-today" "$(pkg minimax-h3 generate)" "$@" probe_butt:=true
    else cell "jp-$name-new" "$(pkg minimax-h3 generate)" "$@"; fi
    run_outputs "$OUT" "jp-$name-$arm" > "jp-$name-$arm.outputs" 2>> join-pairs.err || true
    JP+=("jp-$name-$arm")
  done
}
jp music "${JP_MUSIC[@]}"
jp tone "${JP_TONE[@]}"
jp speech "${JP_SPEECH[@]}"
jp claps "${JP_CLAPS[@]}"
# Every take is on this runner: the pod ends here.
end_it
# What the new join's session logged at each seam, as a record (sound-joins.py), and each take's plan and file.
python3 -I "$HERE/sound-joins.py" "$OUT" "${JP[@]}" > join-pairs.json 2>> join-pairs.err
for jp_cell in "${JP[@]}"; do
  log "$jp_cell: $(run_get "$OUT" "$jp_cell" .status) $(grep -o 'h3 shot segments[^"]*' "$jp_cell.ndjson" 2> /dev/null | head -1 | cut -c1-300) $(grep -cE '\.mp4$' "$jp_cell.outputs" 2> /dev/null) mp4"
done
expect join-pairs "$(n=0; for jp_cell in "${JP[@]}"; do ! grep -qE '\.mp4$' "$jp_cell.outputs" 2> /dev/null || n=$((n + 1)); done; [ "$n" = "${#JP[@]}" ] && echo PASS || echo FAIL)" \
  "takes exported: $(cat jp-*.outputs 2> /dev/null | grep -cE '\.mp4$') of ${#JP[@]}"
