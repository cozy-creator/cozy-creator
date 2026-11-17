# render-on-new (cl-1066; Paul: "`--on new`: `run` first, then `render`"): `cozy render box --on new:$SKU -y` on a runner with no
# GPU, as a person types it: the render's one rental quoted (POST /v1/projects/{id}/render-quote) and agreed, ordered by Calcifer, and the
# box's take made there and landed in the project. A weightless still (e2e's draw.py), so the cheapest product answers it. The rental
# is this runner's Calcifer's order: the guard reads what the Hub bills for it, and the trap ends it and checks the Hub lists it ended.
# One rental of $SKU, ordered by the render. It takes no params.
. "$(dirname "$0")/lib.sh"
E2E=$(cd "$(dirname "$0")/../../e2e/cli" && pwd)
[ -f "$E2E/project/draw.py" ] || fail "no e2e/cli/project/draw.py"
J() { jq -e "$@" > /dev/null; }
# What this runner's Calcifer ordered is this session's: it started with none.
trap 'code=$?; claim; (exit "$code"); finish' EXIT
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
jq -n --arg e "$E2E" '[{name: "box", tool: "\($e)/project/draw.py/draw", kind: "image", fields: {instructions: "a lighthouse at dusk"}}]' \
  | run_page boxes > "$OUT/boxes.txt" 2>&1 || fail "the box: $(tail -3 "$OUT/boxes.txt")"
# A pod's boot, the still's install there and its call: 20 minutes is plenty.
quote 1200
GUARD_ALSO=own guard

say "render box --on new:$SKU"
t0=$SECONDS
(cd "$RUN_PROJECT" && exec timeout 1800 cozy --json -y render box --on "new:$SKU") > "$OUT/render.ndjson" 2> "$OUT/render.err" & pid=$!
BG="$BG $pid"
while kill -0 "$pid" 2> /dev/null; do claim; sleep 5; done
wait "$pid"; code=$?
claim
took=$((SECONDS - t0))
say "the render exited $code after ${took}s: $(grep -E '"(quote|said)"' "$OUT/render.ndjson" | head -c 800)"
[ "$code" = 0 ] || fail "render --on new:$SKU exited $code: $(tail -c 500 "$OUT/render.err") $(tail -c 500 "$OUT/render.ndjson")"
[ "${#RIDS[@]}" = 1 ] || fail "the render rented ${#RIDS[@]} machines, not one: ${RIDS[*]:-none}"
RID=${RIDS[0]}
cozy machine show "$RID" --json > "$OUT/show.json" 2>&1
NAME=$(jq -r '.name // empty' "$OUT/show.json")
say "its rental $RID is $NAME: $(jq -c '{state, sku: .rental.sku, usd_micros_per_hour: .rental.usd_micros_per_hour, provider: .rental.provider}' "$OUT/show.json")"
J '.quote.usd_micros_per_hour > 0' <(grep '"quote"' "$OUT/render.ndjson" | head -1) || fail "the render showed no quote: $(head -c 300 "$OUT/render.ndjson")"
# The take landed, from a run of the still on the rental.
run_page asset box > "$OUT/asset.json" 2>&1
J '.versions | length == 1' "$OUT/asset.json" || fail "box has no take: $(tail -c 400 "$OUT/asset.json")"
(cd "$RUN_PROJECT" && cozy --json run ls --limit 0) > "$OUT/runs.json" 2>&1
jq -c '[.runs[] | {number, name, target, machine, status}]' "$OUT/runs.json" | tee -a "$R"
ran=$(jq -c '[.runs[] | select((.target // "") | test("^\\S+/draw$"))][0] // {}' "$OUT/runs.json")
J --arg n "$NAME" '.status == "completed" and .machine == $n' <<< "$ran" || fail "the still's run isn't a completed run on $NAME: $ran"
file=$(jq -r '[.versions[-1].files[]?.path // empty][0] // empty' "$OUT/asset.json")
say "PASS: run $(jq -r '.name // .number' <<< "$ran") made box take 1 on $NAME ($RID, $SKU) in ${took}s from the render's start${file:+, landed as $file}"
