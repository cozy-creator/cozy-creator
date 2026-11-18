# sound-joins.sh, a --cells hook, proof only (cl-1210; `-f cells=release-gate/sound-joins.sh -f packages_ref=<proof/cl-1210-sound-probe>`):
# the measurement before a join of a shot's sound is designed. On the packages probe branch (never merged) each segment after the
# first logs, for its seam, how alike segment k's tail and its own rendering of that carried second are, and what a switch or an
# equal-power fade of them at the overlap's middle does (`h3 probe seam`); every segment logs how far back from a cut its sound
# decode changes (`h3 probe reach`). Three shots: incremental-video's lighthouse at its two seeds (44275320398, ambient;
# 44284366904, a hard cut at 8.0 s) and a man speaking across a seam. sound-joins.json holds every line, by shot.
ONLY=  # the suites' --only chose their cells, not these
SJ_LIGHT=(summary="A lighthouse on a rocky coast at dusk, the sea rising." soundscape="Waves breaking, wind, gulls." turbo:=true
  "beats:=$(jq -cn '[{duration: 8, description: "[Shot 1] The beam sweeps over the dark sea as waves roll in and break on the rocks."},
    {duration: 8, description: "The camera rises along the lighthouse as the wind picks up and spray climbs the tower."},
    {duration: 8, description: "From the gallery the camera looks out to sea, where a ship passes under the beam."}]')")
SJ_SPEECH=(summary="An old fisherman on a wooden pier at dusk talks to the camera." soundscape="His voice, close; water lapping under the pier." turbo:=true
  "beats:=$(jq -cn '[{duration: 8, description: "[Shot 1] The fisherman looks into the camera and says: \"The sea was calm this morning, but the wind is turning now, you can feel it on your face.\""},
    {duration: 8, description: "He points at the horizon and keeps talking: \"See those clouds out there? We will have rain by dark, and every boat is coming in early.\""}]')")

if [ -z "$PKGS" ]; then
  expect sound-joins FAIL "needs packages_ref, the probe branch"
  return 0
fi
cell sj-ambient "$(pkg minimax-h3 generate)" "${SJ_LIGHT[@]}" seed:=2046869431
cell sj-cut "$(pkg minimax-h3 generate)" "${SJ_LIGHT[@]}" seed:=1337077874
cell sj-speech "$(pkg minimax-h3 generate)" "${SJ_SPEECH[@]}" seed:=$((R + 1211))
# Everything is in the runs' logs: the pod ends here.
end_it
python3 -I "$HERE/sound-joins.py" "$OUT" sj-ambient sj-cut sj-speech > sound-joins.json 2> sound-joins.err
log "sound-joins: $(jq -c . sound-joins.json 2> /dev/null || tail -c 300 sound-joins.err)"
expect sound-joins "$(jq -r 'if ([.[] | .seams | length] | add) >= 5 and ([.[] | .reach | length] | add) >= 8 then "PASS" else "FAIL" end' sound-joins.json 2> /dev/null || echo FAIL)" \
  "$(jq -c 'map_values({status, seams: (.seams | length), reach: (.reach | length)})' sound-joins.json 2> /dev/null)"
