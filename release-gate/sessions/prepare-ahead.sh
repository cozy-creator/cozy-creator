# prepare-ahead (cl-905, Paul 11-12: "this video node should be trying to download minimax h3 immediately, rather than sitting there
# waiting"; "everything should overlap as much as possible"): one cold rental, a project's picture and the video box that mentions it,
# rendered together through this computer's Calcifer (its walker) on the rental alone. The picture's run is sent at once. The video
# waits for its take, and its fetch ahead (a warm run) installs H3 and downloads its weights behind the picture's, in the machine's
# install and download lines. When the take lands, H3's run starts on weights already there. prepare-ahead.py reads the runs' records,
# the board and TensorD's log (each install turn, each object's start and landing with its order) into timeline.json: each job's spans
# in each line, seconds from the ask, end to end against the longest single chain, and the wait H3 saved. The session's cozy (a dev
# build of the RC) puts its TensorD on the pod; the pod keeps its image's released Runtime, whose kernels the times need (RUNTIME_WHEEL
# is a pure wheel built from source, with none: a pod on it runs slowly and says nothing). params:
#   picture   the picture's tool (default qwen-image-2/image), at `megapixels` (default 1)
#   video     the video's tool (default minimax-h3/generate, its default lane and turbo), `seconds` long (default 15)
. "$(dirname "$0")/lib.sh"
PICTURE=$(param picture qwen-image-2/image) MP=$(param megapixels 1) VIDEO=$(param video minimax-h3/generate) SECONDS_LONG=$(param seconds 15)
jq -n --arg picture "$PICTURE" --argjson mp "$MP" --arg video "$VIDEO" --argjson seconds "$SECONDS_LONG" '[
  {name: "pier", tool: $picture, kind: "image", fields: {megapixels: $mp, aspect_ratio: "16:9",
    instructions: "A weathered wooden pier at dusk, warm lamps along its rail, gulls overhead and a calm grey sea. Photo.",
    description: "A weathered wooden pier at dusk, warm lamps along its rail."}},
  {name: "walk", tool: $video, kind: "video", fields: {aspect_ratio: "16:9",
    summary: "A woman in a yellow raincoat walks the length of <pier>.",
    soundscape: "Waves lapping at the posts, gulls, footsteps on wet wood.",
    beats: [{duration: $seconds, description: "A tracking shot follows a woman in a yellow raincoat as she walks along <pier> toward the lamps at its far end, gulls circling overhead."}]}}]' > "$OUT/boxes.json"
run_page boxes "$OUT/boxes.json" > "$OUT/boxes.txt" 2>&1 || fail "the project's boxes: $(tail -3 "$OUT/boxes.txt")"
say "the session's cozy: $(cozy --version)"
quote 2700
rent

step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod refused the session's TensorD"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v, its image's Runtime $(cozy machine show "$NAME" --json | jq -r '.status.runtime // empty')"

# The board as the page sees it, every 5 s: when the video's fetch ahead began, and what each box said.
( while :; do printf '{"at_ms":%s,"board":%s}\n' "$(date +%s%3N)" "$(run_page board 2> /dev/null || echo null)" >> "$OUT/board.ndjson"; sleep 5; done ) & BG=$!
ASK=$(date +%s%3N)
say "step render: pier and walk asked together on $NAME"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" pier walk > "$OUT/render.txt" 2>&1; rendered=$?
kill "$BG" 2> /dev/null; BG=''
tail -4 "$OUT/render.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"

(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 40) > "$OUT/runs.json" 2>&1
for n in $(jq -r --argjson a "$ASK" '.runs[]? | select(.created_at_ms >= $a - 60000) | .number' "$OUT/runs.json"); do
  (cd "$RUN_PROJECT" && cozy run watch "$n" --once --json --full) > "$OUT/run-$n.json" 2>&1
done
# The whole log since the rental began (the default is its last 200 lines), and the transfer log beside it.
cozy machine logs "$NAME" --lines 0 --since 2h > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
cozy machine logs "$NAME" tensorfs --lines 0 --since 2h > "$OUT/tensorfs.log" 2>&1 || true
cozy machine show "$NAME" --json > "$OUT/show-after.json" 2>&1
tail -n 20000 "$HOME/.cozy/calcifer/calcifer.log" > "$OUT/calcifer.log" 2> /dev/null || true
for a in pier walk; do run_page asset "$a" > "$OUT/asset-$a.json" 2>&1; done
python3 "$(dirname "$0")/prepare-ahead.py" "$OUT" "$ASK" "${PICTURE%%/*}" "${VIDEO%%/*}" > "$OUT/timeline.json" 2> "$OUT/timeline.failed"
cat "$OUT/timeline.json" >> "$R"
[ "$rendered" = 0 ] || fail "the render didn't land both takes (exit $rendered): $(tail -c 400 "$OUT/render.txt")"
while read -r why; do say "FAIL: $why"; RC=1; done < "$OUT/timeline.failed"
[ "$RC" = 0 ] && say "PASS: the video's fetch ahead downloaded H3 behind the picture's weights, while the picture rendered, and H3 started on them"
