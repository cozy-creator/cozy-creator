# coming_machine (cl-915, Paul 11-12: "the planner … should take into account what machines are coming online"): a project's machines
# are a stand-in (stand-c, another user's cozy on this runner) and a rental on the isolated Hub's docker provider, which never serves
# here (its Hub's ingress is on loopback), so it stays on its way: a typical boot from its order, then its preparation and a minute's
# run, as its Hub has no times of them (about 6 min in all). stand-c first runs the `wait` tool for 8 min, so it knows that's what the
# tool takes there. A still of another tool (`slow`, which stand-c hasn't run: its install and a minute) finishes sooner on stand-c and
# goes there at once: a rental that will be later doesn't hold it. `wait` again would take stand-c 8 min, so it waits for the rental,
# nothing sent, its box saying "waits for <rental>, ready in …"; the rental ended, it goes to stand-c. `--on new` of a run stand-c
# finishes sooner than a new rental would rents nothing and runs there. On a GitHub runner the machines run CPU work side by side, so
# stand-c is late by its own measured time, not by a queue: placement compares the same finish. Its own shard (`coming`), by hand: it
# takes about 15 min.
coming_machine() {
step "coming machine (cl-915): a job waits for a rental on its way that finishes it sooner, never for one that's later"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] && [ -n "${E2E_HUB_LOG:-}" ] \
  || { echo "coming machine: its stand-in is another user of a fresh GitHub runner, and it rents on the isolated Hub"; return; }
COMING_RENTAL=''
coming_cell
# As it found them: no rental, no stand-in, signed out (rental_end signs in after it).
[ -z "$COMING_RENTAL" ] || timeout 120 cozy --json machine end "$COMING_RENTAL" --force -y > "$OUT/coming-end-after.json" 2>&1 || true
timeout 60 "$COZY" --json machine end stand-c --force -y > "$OUT/stand-c-end.json" 2>&1 || true
cozy --json logout > "$OUT/coming-logout.json" 2>&1 || true
}

# coming_go <asset> <machines, JSON>: renders it as the page does, sending the project's machines.
coming_go() {
  project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: coming-$1-$RANDOM$RANDOM" \
    -d "{\"names\":[\"$1\"],\"on\":$2}" > "$OUT/coming-go-$1.json" 2>&1 || check "coming: render $1: $(tail -c 300 "$OUT/coming-go-$1.json")"
}

# coming_done <asset> <seconds>: its job on the board done within that.
coming_done() {
  local job=''
  for _ in $(seq $(($2 * 2))); do
    job=$(status "$P" | jq -c --arg a "$1" '.board.jobs[]? | select(.asset == $a)' || true)
    J '.state == "done" or .state == "failed"' <<< "$job" && break
    sleep 0.5
  done
  J '.state == "done"' <<< "$job" || check "coming: $1 isn't done: $job"
}

coming_cell() {
local P=$OUT/coming tool=$here/project/waits.py/wait rental='' name='' pid board
cozy --json machine show local > "$OUT/set-local.json" || { check "coming: machine show local"; return; }
stand_in stand-c || return
signin "$E2E_HUB_EMAIL" > "$OUT/coming-login.json" || { check "coming: cozy login: $(tail -c 300 "$OUT/coming-login.json")"; return; }
cozy --json new --unencrypted "$P" > "$OUT/coming-new.json" 2>&1 || { check "coming: new: $(tail -c 300 "$OUT/coming-new.json")"; return; }
jq -n --arg wait "$tool" --arg slow "$here/project/tool.py/slow" --arg r "$RANDOM" '[
  {name: "learned", tool: $wait, kind: "image", fields: {instructions: "What stand-c learns the tool takes, \($r).", megapixels: 0.25, seconds: 480}},
  {name: "small", tool: $slow, kind: "image", fields: {instructions: "Sooner on stand-c than on the rental, \($r).", seconds: 1}},
  {name: "large", tool: $wait, kind: "image", fields: {instructions: "Sooner on the rental than on stand-c, \($r).", megapixels: 0.25, seconds: 1}}]' | boxes_in "$P" || return
coming_go learned '["stand-c"]'
coming_done learned 900

# On its way: ordered, the follower killed as a closed terminal kills it (the order stands), its host named by its Hub.
cozy --json machine rent cpu -y > "$OUT/coming-rent.ndjson" 2> "$OUT/coming-rent.err" & pid=$!
for _ in $(seq 180); do
  rental=$(jq -r 'select(.type? == "rental.accepted") | .rental_id // empty' "$OUT/coming-rent.ndjson" 2> /dev/null | tail -1 || true)
  [ -n "$rental" ] || ! kill -0 "$pid" 2> /dev/null || { sleep 1; continue; }
  break
done
kill -KILL "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
[ -n "$rental" ] || { check "coming: machine rent cpu: no rental accepted: $(tail -c 300 "$OUT/coming-rent.ndjson") $(tail -c 300 "$OUT/coming-rent.err")"; return; }
for _ in $(seq 120); do
  name=$(cozy --json machine ls 2> /dev/null | jq -r --arg r "$rental" '.machines[] | select(.rental_id == $r and .placeholder == null and .state == "booting") | .name' || true)
  [ -z "$name" ] || break
  sleep 0.5
done
[ -n "$name" ] || { check "coming: $rental was never named on its way: $(cozy --json machine ls | jq -c '[.machines[] | {name, state, placeholder}]')"; COMING_RENTAL=$rental; return; }
COMING_RENTAL=$name
in_project "$P" --json machine use "stand-c,$name" > "$OUT/coming-use.json" 2>&1 || { check "coming: machine use stand-c,$name: $(tail -c 300 "$OUT/coming-use.json")"; return; }

# Later on the rental: it goes to stand-c at once.
coming_go small "[\"stand-c\",\"$name\"]"
coming_done small 300
in_project "$P" --json run ls --limit 0 > "$OUT/coming-runs.json"
J '[.runs[] | select(.kind == "call" and .machine == "stand-c" and .status == "completed")] | length == 2' "$OUT/coming-runs.json" \
  || check "coming: small, sooner on stand-c, didn't run there: $(jq -c '[.runs[] | {number, machine, status}]' "$OUT/coming-runs.json")"

# Sooner on the rental: held for it, saying so, sent nowhere.
coming_go large "[\"stand-c\",\"$name\"]"
for _ in $(seq 120); do
  board=$(status "$P" | jq -c '.board.jobs[]? | select(.asset == "large")' || true)
  J --arg m "$name" '(.why // "" | startswith("waits for \($m), ready ")) and .machine == $m and .run == null' <<< "$board" && break
  sleep 0.5
done
echo "coming: large, held: $board"
J --arg m "$name" '(.why // "" | startswith("waits for \($m), ready ")) and .machine == $m and .run == null' <<< "$board" \
  || check "coming: large, sooner on $name, isn't held for it: $board"
sleep 12
in_project "$P" --json run ls --limit 0 > "$OUT/coming-held.json"
J '[.runs[] | select(.kind == "call")] | length == 2' "$OUT/coming-held.json" \
  || check "coming: a run was sent for large while its machine is on its way: $(jq -c '[.runs[] | {number, machine, status}]' "$OUT/coming-held.json")"

# The rental ended: large goes to stand-c.
timeout 120 cozy --json machine end "$name" --force -y > "$OUT/coming-end.json" 2>&1 || check "coming: machine end $name: $(tail -c 300 "$OUT/coming-end.json")"
COMING_RENTAL=''
coming_done large 300
in_project "$P" --json run ls --limit 0 > "$OUT/coming-after.json"
J '[.runs[] | select(.kind == "call" and .machine == "stand-c" and .status == "completed")] | length == 3' "$OUT/coming-after.json" \
  || check "coming: large didn't go to stand-c once the rental ended: $(jq -c '[.runs[] | {number, machine, status}]' "$OUT/coming-after.json")"

# `--on new`, stand-c sooner than a new rental: nothing rented (no quote to agree to), it runs there and says why.
in_project "$P" --json run minimax-h3-tools/echo text=coming --on new > "$OUT/coming-new-run.ndjson" 2> "$OUT/coming-new-run.err" \
  || check "coming: run --on new: $(tail -c 300 "$OUT/coming-new-run.ndjson") $(tail -c 300 "$OUT/coming-new-run.err")"
grep -q 'stand-c finishes it .*nothing is rented' "$OUT/coming-new-run.ndjson" "$OUT/coming-new-run.err" \
  || check "coming: run --on new didn't say stand-c finishes it sooner: $(tail -c 300 "$OUT/coming-new-run.err")"
in_project "$P" --json run ls --limit 0 > "$OUT/coming-new-runs.json"
J '[.runs[] | select(.target | test("/echo"))] | .[0].machine == "stand-c"' "$OUT/coming-new-runs.json" \
  || check "coming: run --on new didn't run on stand-c: $(jq -c '[.runs[] | {number, target, machine, status}]' "$OUT/coming-new-runs.json")"
cozy --json machine ls | J '[.machines[] | select(.kind == "rental" and .state != "ended")] | length == 0' \
  || check "coming: run --on new rented: $(cozy --json machine ls | jq -c '[.machines[] | {name, kind, state}]')"
}
