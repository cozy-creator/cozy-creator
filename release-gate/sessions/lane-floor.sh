# lane-floor (cl-1076): a model resolves to its 8-bit lane on a card that runs one, however little of the card is free. Paul,
# 11-06 (xs-154 Q1): at the default 8-bit floor "the 8-bit lane always serves; memory never lowers it". Paul, 11-16, on an
# RTX 5090 that held H3's nvfp4 lane: "we should be downloading the fp8 paths". TensorD asked the Hub with the card's free
# bytes, and the int8 components H3's calls read (52 GB) fit no consumer card's, so every Blackwell card under 52 GB was served
# nvfp4. One card with FP4 tensor cores and less memory than that, the session's cozy as its TensorD: `target` runs until the
# model's header lands, its lane is read from `machine show`, and the run is canceled. Red before the fix (nvfp4), green after.
# params:
#   target   the function that reads the model (minimax-h3/generate)
#   model    the model whose lane is judged (minimaxai/minimax-h3)
#   want     the lane it must resolve to (int8)
. "$(dirname "$0")/lib.sh"
T=$(param target minimax-h3/generate) M=$(param model minimaxai/minimax-h3) WANT=$(param want int8)
jq -n --argjson s "$RANDOM$RANDOM" '{beats: [{description: "A lighthouse on a sea cliff at dusk, waves breaking below.", duration: 5}], seed: $s}' \
  > "$OUT/request.json"
quote 1800
rent

jq -e '.status.gpus | length == 1 and (.[0].family | ltrimstr("sm") | tonumber) >= 100' "$OUT/show.json" > /dev/null ||
  fail "not one card with FP4 tensor cores: $(jq -c '[.status.gpus[] | {name, family}]' "$OUT/show.json")"
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
(cd "$RUN_PROJECT" && exec cozy run "$T" --input "$OUT/request.json" --gpus=1 --on "$NAME") > "$OUT/run.txt" 2>&1 & follower=$!
lane='' t0=$(date +%s)
while [ -z "$lane" ] && [ $(($(date +%s) - t0)) -lt 1200 ]; do
  sleep 5
  lane=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r --arg m "$M" '[.status.models[]? | select(.repository == $m) | .checkpoints[].lane] | first // empty')
  kill -0 "$follower" 2> /dev/null || [ -n "$lane" ] || break
done
RUN=$(awk '$1=="run" && $2 ~ /^[0-9]+$/ {print $2; exit}' "$OUT/run.txt")
if [ -n "$RUN" ]; then
  (cd "$RUN_PROJECT" && cozy run cancel "$RUN" -y) >> "$OUT/cancel.txt" 2>&1
  (cd "$RUN_PROJECT" && cozy run watch "$RUN" --once --json --full) > "$OUT/run.json" 2>&1
fi
kill "$follower" 2> /dev/null; wait "$follower" 2> /dev/null
cozy machine show "$NAME" --json > "$OUT/models.json" 2>&1
say "run ${RUN:-none}: $M resolved to ${lane:-nothing} on $(jq -r '.status.gpus[0] | "\(.name) (\(.family), \(.memory_bytes / 1e9 | floor) GB)"' "$OUT/show.json")"
say "its warnings: $(jq -c '[.. | objects | select(has("message")) | .message | select(type == "string" and test("lane|fits"))] | unique' "$OUT/run.json" 2> /dev/null)"
[ -n "$lane" ] || fail "$M's header never landed: $(tail -3 "$OUT/run.txt" | cut -c1-300)"
[ "$lane" = "$WANT" ] || fail "$M resolved to $lane, not $WANT"
say "PASS: $M resolved to $WANT"
