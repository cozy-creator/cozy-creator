# plan-two-pods (cl-967, Paul 11-12: "calcifer could compute the quote on its end, if it has sufficient knowledge of each machine, and
# use that to find the fastest path in its planner"): two rentals of one SKU, both on the session's TensorD. `warm` renders an H3
# shot first, so it holds H3's weights and its time there is measured; `cold`, rented after it, holds nothing. A second shot, rendered through this
# computer's Calcifer on both, is planned from what they push: it goes to warm, and its quote (Calcifer's log: `goes to <warm>: …`)
# is set against its run's actual end, within 20% (or 30 s). Both pods end and are seen gone. params:
#   seconds  each shot's length (default 15)
. "$(dirname "$0")/lib.sh"
SECONDS_LONG=$(param seconds 15)
shot() {
  jq -n --arg name "$1" --argjson seconds "$SECONDS_LONG" --arg words "$2" '{name: $name, tool: "minimax-h3/generate", kind: "video",
    fields: {aspect_ratio: "16:9", summary: $words, soundscape: "Wind and distant gulls.",
      beats: [{duration: $seconds, description: $words}]}}'
}
jq -s . <(shot first "A lighthouse on a cliff at dawn, waves breaking below, the camera slowly circling it.") \
  <(shot second "A fishing boat leaving a harbour at dusk, gulls following it, lamps coming on along the quay.") > "$OUT/boxes.json"
run_page boxes "$OUT/boxes.json" > "$OUT/boxes.txt" 2>&1 || fail "the project's boxes: $(tail -3 "$OUT/boxes.txt")"
quote 3600 "$SKU" "$SKU"
rent; WARM=$NAME
step tensord machine update "$WARM" --tensord "$(command -v cozy)" -y || fail "$WARM refused the session's TensorD"
say "step first: on $WARM alone, so it holds H3 and has timed it"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" first > "$OUT/first.txt" 2>&1 || fail "the first shot: $(tail -c 400 "$OUT/first.txt")"
# The cold one rented only now: nothing of it is paid for while the first shot renders.
rent; COLD=$NAME
step tensord machine update "$COLD" --tensord "$(command -v cozy)" -y || fail "$COLD refused the session's TensorD"
ASK=$(date +%s%3N)
say "step second: on $WARM and $COLD, where it finishes soonest"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM,$COLD" second > "$OUT/second.txt" 2>&1 || fail "the second shot: $(tail -c 400 "$OUT/second.txt")"
tail -n 20000 "$HOME/.cozy/calcifer/calcifer.log" > "$OUT/calcifer.log" 2> /dev/null || true
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 20) > "$OUT/runs.json" 2>&1
run=$(jq -c --argjson a "$ASK" '[.runs[]? | select(.kind == "call" and .created_at_ms >= $a)] | first // empty' "$OUT/runs.json")
[ -n "$run" ] || fail "no run of the second shot: $(head -c 400 "$OUT/runs.json")"
went=$(jq -r .machine <<< "$run")
said=$(grep -E "goes to $went: " "$OUT/calcifer.log" | tail -1)
say "planned: ${said:-no placement line}"
[ "$went" = "$WARM" ] || fail "the second shot went to $went, not $WARM, which holds H3"
quoted=$(grep -oE "$went finishes in [0-9]+ s" <<< "$said" | grep -oE '[0-9]+' | head -1)
actual=$(( ($(jq -r .settled_at_ms <<< "$run") - ASK) / 1000 ))
bound=$(( actual / 5 > 30 ? actual / 5 : 30 ))
say "quoted ${quoted:-?} s, finished in $actual s (bound $bound s)"
[ -n "$quoted" ] && [ $(( quoted > actual ? quoted - actual : actual - quoted )) -le "$bound" ] || fail "its quote ${quoted:-none} s is off its $actual s by more than $bound s"
say "PASS: the plan sent the shot to the pod holding H3, and its quote held"
