# plan-two-pods (cl-967, Paul 11-12: "calcifer could compute the quote on its end, if it has sufficient knowledge of each machine, and
# use that to find the fastest path in its planner"): two rentals of one SKU, both on the session's TensorD. `warm` renders two H3
# shots: the first, its first call there, records H3's cold time (its plan compiling); the second its warm time, and leaves H3 held.
# `cold`, rented after them, holds nothing. A third shot, rendered through this computer's Calcifer on both, is planned from what they
# push: it goes to warm, quoted from its warm time (Calcifer's log: `goes to <warm>: <warm> finishes in N s (Measured, warm)`), and
# ends within 20% (or 30 s) of that. Both pods end and are seen gone. params:
#   seconds  each shot's length (default 15)
. "$(dirname "$0")/lib.sh"
SECONDS_LONG=$(param seconds 15)
shot() {
  jq -n --arg name "$1" --argjson seconds "$SECONDS_LONG" --arg words "$2" '{name: $name, tool: "minimax-h3/generate", kind: "video",
    fields: {aspect_ratio: "16:9", summary: $words, soundscape: "Wind and distant gulls.",
      beats: [{duration: $seconds, description: $words}]}}'
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
quote 3600 "$SKU" "$SKU"
rent_ok; WARM=$NAME
step tensord machine update "$WARM" --tensord "$(command -v cozy)" -y || fail "$WARM refused the session's TensorD"
say "step first: on $WARM alone, its first H3 call there: the cold time"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" first > "$OUT/first.txt" 2>&1 || fail "the first shot: $(tail -c 400 "$OUT/first.txt")"
say "step second: on $WARM alone again: the warm time, H3 left held"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" second > "$OUT/second.txt" 2>&1 || fail "the second shot: $(tail -c 400 "$OUT/second.txt")"
# The cold one rented only now: nothing of it is paid for while the first two render.
rent_ok; COLD=$NAME
step tensord machine update "$COLD" --tensord "$(command -v cozy)" -y || fail "$COLD refused the session's TensorD"
ASK=$(date +%s%3N)
say "step third: on $WARM and $COLD, where it finishes soonest"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM,$COLD" third > "$OUT/third.txt" 2>&1 || fail "the third shot: $(tail -c 400 "$OUT/third.txt")"
tail -n 20000 "$HOME/.cozy/calcifer/calcifer.log" > "$OUT/calcifer.log" 2> /dev/null || true
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 20) > "$OUT/runs.json" 2>&1
run=$(jq -c --argjson a "$ASK" '[.runs[]? | select(.kind == "call" and .created_at_ms >= $a)] | first // empty' "$OUT/runs.json")
[ -n "$run" ] || fail "no run of the third shot: $(head -c 400 "$OUT/runs.json")"
went=$(jq -r .machine <<< "$run")
said=$(grep -E "goes to $went: " "$OUT/calcifer.log" | tail -1)
say "planned: ${said:-no placement line}"
[ "$went" = "$WARM" ] || fail "the third shot went to $went, not $WARM, which holds H3"
grep -q "$went finishes in [0-9]* s (Measured, warm)" <<< "$said" || fail "the third shot's quote isn't its warm time: ${said:-no placement line}"
quoted=$(grep -oE "$went finishes in [0-9]+ s" <<< "$said" | grep -oE '[0-9]+' | head -1)
actual=$(( ($(jq -r .settled_at_ms <<< "$run") - ASK) / 1000 ))
bound=$(( actual / 5 > 30 ? actual / 5 : 30 ))
say "quoted ${quoted:-?} s, finished in $actual s (bound $bound s)"
[ -n "$quoted" ] && [ $(( quoted > actual ? quoted - actual : actual - quoted )) -le "$bound" ] || fail "its quote ${quoted:-none} s is off its $actual s by more than $bound s"
say "PASS: the plan sent the shot to the pod holding H3, and its quote held"
