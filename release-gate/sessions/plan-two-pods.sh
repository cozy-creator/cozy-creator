# plan-two-pods (cl-967, Paul 11-12: "calcifer could compute the quote on its end, if it has sufficient knowledge of each machine, and
# use that to find the fastest path in its planner"): two rentals of one SKU, both on the session's TensorD. `warm` renders two H3
# shots: the first, its first call there, records H3's cold time (its plan compiling); the second its warm time, and leaves H3 held.
# `cold`, rented after them, holds nothing. A third shot, rendered through this computer's Calcifer on both, is planned from what they
# push: it goes to warm, quoted from its warm time (Calcifer's log: `goes to <warm>: <warm> finishes in N s (Measured, warm)`), and
# ends within 20% (or 30 s) of that. Each shot is planned (its `goes to` line; none `isn't planned`). Both pods end and are seen gone.
# params:
#   seconds  each H3 shot's length (default 15)
#   tool     the shots' tool (default minimax-h3/generate); another is an image box with a prompt, e.g. qwen-image-2/generate_turbo
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
# Both pods for 40 min, which overstates it: the cold one runs about 10 of them.
quote 2400 "$SKU" "$SKU"
rent_ok; WARM=$NAME
step tensord machine update "$WARM" --tensord "$(command -v cozy)" -y || fail "$WARM refused the session's TensorD"
say "step first: on $WARM alone, its first $TOOL call there: the cold time"
timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$WARM" first > "$OUT/first.txt" 2>&1 || fail "the first shot: $(tail -c 400 "$OUT/first.txt")"
say "step second: on $WARM alone again: the warm time, its model left held"
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
# Each shot's plan, as Calcifer's log says it: the third's is the line at or after its ask.
grep -E "goes to [a-z0-9-]+: |isn't planned: " "$OUT/calcifer.log" > "$OUT/planned.log" || true
sed 's/^/planned: /' "$OUT/planned.log" | tee -a "$R"
! grep -q "isn't planned: " "$OUT/planned.log" || fail "a shot wasn't planned: $(grep "isn't planned: " "$OUT/planned.log" | head -1)"
[ "$(grep -c "goes to $WARM: " "$OUT/planned.log")" -ge 3 ] || fail "not each of the three shots has a plan line going to $WARM"
since=$(date -u -d "@$((ASK / 1000))" +%Y-%m-%dT%H:%M:%S)
said=$(awk -v t="$since" 'substr($1, 1, 19) >= t' "$OUT/planned.log" | grep -E "goes to $went: " | tail -1)
say "planned: ${said:-no placement line}"
[ "$went" = "$WARM" ] || fail "the third shot went to $went, not $WARM, which holds its model"
grep -q "$went finishes in [0-9]* s (Measured, warm)" <<< "$said" || fail "the third shot's quote isn't its warm time: ${said:-no placement line}"
quoted=$(grep -oE "$went finishes in [0-9]+ s" <<< "$said" | grep -oE '[0-9]+' | head -1)
actual=$(( ($(jq -r .settled_at_ms <<< "$run") - ASK) / 1000 ))
bound=$(( actual / 5 > 30 ? actual / 5 : 30 ))
say "quoted ${quoted:-?} s, finished in $actual s (bound $bound s)"
[ -n "$quoted" ] && [ $(( quoted > actual ? quoted - actual : actual - quoted )) -le "$bound" ] || fail "its quote ${quoted:-none} s is off its $actual s by more than $bound s"
say "PASS: the plan sent the shot to the pod holding H3, and its quote held"
