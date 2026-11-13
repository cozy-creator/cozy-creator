# plan-two-pods (cl-967, Paul 11-12: "calcifer could compute the quote on its end, if it has sufficient knowledge of each machine, and
# use that to find the fastest path in its planner"): two rentals of one SKU, both on the session's TensorD. `warm` renders two H3
# shots: the first, its first call there, records H3's cold time (its plan compiling); the second its warm time, and leaves H3 held.
# `cold`, rented after them, holds nothing. A third shot, rendered through this computer's Calcifer on both, is planned from what they
# push: it goes to warm, quoted from its warm time (its run's `placed`: warm, `measured`, `warm: true`), and ends within 20% (or
# 30 s) of that. Each shot's run says where the plan placed it, on warm. Both pods end and are seen gone.
# params:
#   seconds  each H3 shot's length (default 15)
#   tool     the shots' tool (default minimax-h3/generate); another is an image box with a prompt, e.g. qwen-image-2/generate_turbo
#   paid     the seconds of both pods the cap must buy before renting (default 2400: H3's; the cold one runs about a quarter of them)
. "$(dirname "$0")/lib.sh"
SECONDS_LONG=$(param seconds 15)
TOOL=$(param tool minimax-h3/generate)
shot() {
  case $TOOL in
    minimax-h3/*) jq -n --arg name "$1" --arg tool "$TOOL" --argjson seconds "$SECONDS_LONG" --arg words "$2" '{name: $name, tool: $tool,
      kind: "video", fields: {aspect_ratio: "16:9", summary: $words, soundscape: "Wind and distant gulls.",
        beats: [{duration: $seconds, description: $words}]}}' ;;
    *) jq -n --arg name "$1" --arg tool "$TOOL" --arg words "$2" '{name: $name, tool: $tool, kind: "image", fields: {prompt: $words, megapixels: 1}}' ;;
  esac
}
jq -s . <(shot first "A lighthouse on a cliff at dawn, waves breaking below, the camera slowly circling it.") \
  <(shot second "A heron wading through reeds at sunrise, mist over the water, the camera low beside it.") \
  <(shot third "A fishing boat leaving a harbour at dusk, gulls following it, lamps coming on along the quay.") > "$OUT/boxes.json"
run_page boxes "$OUT/boxes.json" > "$OUT/boxes.txt" 2>&1 || fail "the project's boxes: $(tail -3 "$OUT/boxes.txt")"
# rent_ok: a rental, as `rent`, on a driver no TensorD update is known to hang on: 610.57.04 holds a thread in uvm, so the update
# never finishes (cl-977). Such a pod is ended at once, before its update, and another rented, at most three times.
rent_ok() {
  local tries=0 driver
  while :; do
    rent
    driver=$(jq -r '.status.gpus[0].driver // empty' "$OUT/show$((${#RIDS[@]} - 1)).json")
    [ "$driver" = 610.57.04 ] || return 0
    say "$NAME has driver $driver, where a TensorD update hangs (cl-977): it ends now, and another is rented"
    cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1
    tries=$((tries + 1))
    [ "$tries" -lt 3 ] || fail "three rentals came with driver $driver"
  done
}
quote "$(param paid 2400)" "$SKU" "$SKU"
rent_ok; WARM=$NAME
step tensord machine update "$WARM" --tensord "$(command -v cozy)" -y || fail "$WARM refused the session's TensorD"
say "step first: on $WARM alone, its first $TOOL call there: the cold time"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" first > "$OUT/first.txt" 2>&1 || fail "the first shot: $(tail -c 400 "$OUT/first.txt")"
say "step second: on $WARM alone again: the warm time, its model left held"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" second > "$OUT/second.txt" 2>&1 || fail "the second shot: $(tail -c 400 "$OUT/second.txt")"
# What warm holds of each model and what its function read (cl-967): what the third shot's quote counts to fetch there.
cozy --json machine show "$WARM" > "$OUT/show-warm.json" 2>&1 || true
jq -r '.status.models[]? | .repository as $m | .checkpoints[]? | "held: \($m) \(.lane) \(.bytes) bytes, \(.held_bytes) held"' "$OUT/show-warm.json" 2> /dev/null | tee -a "$R"
jq -r '.status.times[]? | .function as $f | .reads[]? | "read: \($f) \(.model) \(.bytes) bytes, \(.missing) missing"' "$OUT/show-warm.json" 2> /dev/null | tee -a "$R"
# The cold one rented only now: nothing of it is paid for while the first two render.
rent_ok; COLD=$NAME
step tensord machine update "$COLD" --tensord "$(command -v cozy)" -y || fail "$COLD refused the session's TensorD"
ASK=$(date +%s%3N)
say "step third: on $WARM and $COLD, where it finishes soonest"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM,$COLD" third > "$OUT/third.txt" 2>&1 || fail "the third shot: $(tail -c 400 "$OUT/third.txt")"
# Each shot's run, and where the plan sent it: its record's `placed`, as `cozy run watch --once` reads it from the project.
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 20) > "$OUT/runs.json" 2>&1
jq -c '[.runs[]? | select(.kind == "call")] | sort_by(.created_at_ms) | .[-3:] | .[]' "$OUT/runs.json" > "$OUT/shots.ndjson" 2> /dev/null || true
[ "$(wc -l < "$OUT/shots.ndjson")" -eq 3 ] || fail "not three runs of the shots: $(head -c 400 "$OUT/runs.json")"
while read -r shot; do
  n=$(jq -r .number <<< "$shot")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/run-$n.json" 2>&1 || true
  placed=$(jq -c '.placed // empty' "$OUT/run-$n.json" 2> /dev/null)
  say "run $n ran on $(jq -r .machine <<< "$shot"); placed: ${placed:-nowhere}"
  [ -n "$placed" ] || fail "run $n has no placement on its record: the plan didn't place it"
  [ "$(jq -r .machine <<< "$placed")" = "$WARM" ] || fail "run $n was placed on $(jq -r .machine <<< "$placed"), not $WARM"
done < "$OUT/shots.ndjson"
run=$(tail -1 "$OUT/shots.ndjson")
placed=$(jq -c .placed "$OUT/run-$(jq -r .number <<< "$run").json")
went=$(jq -r .machine <<< "$run")
[ "$went" = "$WARM" ] || fail "the third shot went to $went, not $WARM, which holds its model"
[ "$(jq -r '.created_at_ms >= '"$ASK" <<< "$run")" = true ] || fail "the newest shot run is older than the third shot's ask: $run"
jq -e '.basis == "measured" and .warm == true' <<< "$placed" > /dev/null || fail "the third shot's quote isn't its warm time: $placed"
quoted=$(jq -r '(.finish_at_ms - .quoted_at_ms) / 1000 | floor' <<< "$placed")
actual=$(( ($(jq -r .settled_at_ms <<< "$run") - ASK) / 1000 ))
bound=$(( actual / 5 > 30 ? actual / 5 : 30 ))
say "quoted $quoted s ($(jq -r '"\(.prepare_ms / 1000 | floor) s preparing, \(.fetch_bytes) bytes to fetch"' <<< "$placed")), finished in $actual s (bound $bound s)"
[ $(( quoted > actual ? quoted - actual : actual - quoted )) -le "$bound" ] || fail "its quote $quoted s is off its $actual s by more than $bound s"
say "PASS: the plan sent the shot to the pod holding its model, and its quote held"
