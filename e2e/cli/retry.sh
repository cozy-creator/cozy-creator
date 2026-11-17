# retry_elsewhere (cl-1085, cl-1132). Paul, 11-16: "when you shutdown a machine, and the job ends because of it, you should be given the
# chance to retry it ... we'll try to place that run on a different machine then, or whatever we have available." Paul, 11-17: "the real
# fix should be like 'calcifer has a queue of jobs, it gives that queue of jobs to a machine. If that machine dies or disappears, it
# re-assigns that queued job to another machine.'" Two boxes render on a stand-in machine (another user of this runner, as machine_set's),
# the project's one machine, each held there (waits.py's `held`), and it ends mid-run. Neither fails: each run returns to Calcifer's queue
# as its next attempt ("attempt 1 stopped: stand-r shut down; attempt 2 of 3 …") and completes on local, on the seed it was asked with;
# `cozy render cut` follows it there and exits 0, and the page (web/retry.js) says no failure and shows paged's take land.
retry_elsewhere() {
step "a running job's machine ends: it reruns elsewhere as attempt 2 and completes (cl-1085, cl-1132)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "retry elsewhere: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/retry tool=$here/project/waits.py/held pw=$OUT/playwright chrome cut='' paged='' job pid code=0 name run
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "retry elsewhere: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/retry-npm.log" 2>&1 || { check "retry elsewhere: npm install playwright-core: $(tail -c 300 "$OUT/retry-npm.log")"; return; }
cozy --json machine show local > "$OUT/set-local.json" || { check "retry elsewhere: machine show local: $(head -c 300 "$OUT/set-local.json")"; return; }
stand_in stand-r || return
cozy --json new --unencrypted "$P" > "$OUT/retry-new.json" 2>&1 || { check "retry elsewhere: new: $(tail -c 300 "$OUT/retry-new.json")"; return; }
jq -n --arg tool "$tool" --argjson home "$(id -u)" '["cut", "paged"] | map({name: ., tool: $tool, kind: "image",
  fields: {instructions: "Held on stand-r until it ends: \(.).", megapixels: 0.25, seconds: 600, home: $home}})' | boxes_in "$P" || return
in_project "$P" --json machine use stand-r > "$OUT/retry-use.json" 2>&1 && J '.machines == ["stand-r"]' "$OUT/retry-use.json" \
  || { check "retry elsewhere: machine use stand-r: $(tail -c 300 "$OUT/retry-use.json")"; return; }

# Both rendering there: cut by `cozy render`, paged as the page asks one, each accepted by stand-r.
(in_project "$P" --json render cut > "$OUT/retry-cut.ndjson" 2> "$OUT/retry-cut.err") & pid=$!
project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: retry-$RANDOM$RANDOM" -d '{"names":["paged"],"on":[]}' \
  > "$OUT/retry-paged.json" 2>&1 || check "retry elsewhere: render paged: $(tail -c 300 "$OUT/retry-paged.json")"
for _ in $(seq 300); do
  for name in cut paged; do
    job=$(status "$P" | jq -r --arg n "$name" '[.board.jobs[]? | select(.asset == $n and .run != null)] | last | .run // empty' || true)
    [ -n "$job" ] && project_api "$P" "/runs/$job" 2> /dev/null | J '.status == "in_progress" and .machine == "stand-r"' && printf -v "$name" %s "$job"
  done
  [ -n "$cut" ] && [ -n "$paged" ] && break
  sleep 0.5
done
if [ -z "$cut" ] || [ -z "$paged" ]; then
  check "retry elsewhere: cut and paged weren't both rendering on stand-r: cut '$cut', paged '$paged': $(status "$P" | jq -c '[.board.jobs[]? | {asset, state, run, machine, why}]') $(tail -c 300 "$OUT/retry-cut.err")"
  timeout 60 "$COZY" --json machine end stand-r --force -y > /dev/null 2>&1 || true
  kill "$pid" 2> /dev/null || true
  return
fi
project_api "$P" "/runs/$cut?detail=full" > "$OUT/retry-cut-before.json" 2>&1

# The page open on the project as stand-r ends: no failure said, and paged's take lands.
HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder CHROME=$chrome PLAYWRIGHT=$pw/node_modules/playwright-core COZY=$COZY SHOT=$OUT/retry-page LAST_LOOK=$OUT/retry-page \
  node "$here/web/retry.js" "$P" stand-r "$paged" "$cut" > "$OUT/retry-page.json" 2> "$OUT/retry-page.err" || code=$?
[ "$code" = 0 ] || check "retry elsewhere: the page: $(jq -c '{failures, seen}' "$OUT/retry-page.json" 2> /dev/null | head -c 2000) $(tail -c 500 "$OUT/retry-page.err")"
timeout 60 "$COZY" --json machine end stand-r --force -y > /dev/null 2>&1 || true

# cut, from the CLI: the same run, its second attempt, completed on local on its seed; `cozy render cut` followed it there.
for _ in $(seq 240); do kill -0 "$pid" 2> /dev/null || break; sleep 0.5; done
kill "$pid" 2> /dev/null && check "retry elsewhere: cozy render cut still followed it two minutes after stand-r ended"
code=0; wait "$pid" 2> /dev/null || code=$?
for run in "$cut" "$paged"; do
  project_api "$P" "/runs/$run?detail=full" > "$OUT/retry-$run.json" 2>&1
  name=$(jq -r '.name // empty' "$OUT/retry-$run.json")
  J '.status == "completed" and .machine == "local"' "$OUT/retry-$run.json" \
    || check "retry elsewhere: $name, rendering on stand-r as it ended, didn't complete on local: $(jq -c '{name, machine, status, reason}' "$OUT/retry-$run.json")"
  project_api "$P" "/runs/$run/events" --max-time 10 -H 'Accept: text/event-stream' > "$OUT/retry-said-$run.txt" 2> /dev/null || true
  grep -q 'attempt 1 stopped: stand-r shut down; attempt 2 of 3' "$OUT/retry-said-$run.txt" \
    || check "retry elsewhere: $name's log doesn't say attempt 1 stopped as stand-r shut down and attempt 2 of 3 went elsewhere: $(grep -o '"text":"[^"]*"' "$OUT/retry-said-$run.txt" | tail -5)"
done
[ "$code" = 0 ] || check "retry elsewhere: cozy render cut exited $code: $(tail -c 300 "$OUT/retry-cut.err")"
J --slurpfile was "$OUT/retry-cut-before.json" '.request.seed != null and .request.seed == $was[0].request.seed' "$OUT/retry-$cut.json" \
  || check "retry elsewhere: cut's second attempt didn't keep its seed: $(jq -c '.request.seed' "$OUT/retry-$cut.json"), was $(jq -c '.request.seed' "$OUT/retry-cut-before.json")"
row "$P" cut '.failure' | J '. == null' || check "retry elsewhere: cut failed: $(row "$P" cut '.failure')"
[ "$(made "$P" cut)" -ge 1 ] || check "retry elsewhere: cut has no take"
# Nothing failed, so `cozy run retry` has nothing to do.
name=$(jq -r '.name // empty' "$OUT/retry-$cut.json")
code=0; in_project "$P" --json run retry "$name" > "$OUT/retry-cli.json" 2> "$OUT/retry-cli.err" || code=$?
[ "$code" != 0 ] && grep -q 'run_completed' "$OUT/retry-cli.json" "$OUT/retry-cli.err" \
  || check "retry elsewhere: cozy run retry $name, completed on its second attempt, wasn't refused run_completed: exit $code $(tail -c 300 "$OUT/retry-cli.json") $(tail -c 300 "$OUT/retry-cli.err")"
echo "retry elsewhere: cut $(jq -r '.name' "$OUT/retry-$cut.json") and paged $(jq -r '.name' "$OUT/retry-$paged.json") on $(jq -r '.machine' "$OUT/retry-$cut.json"); the page: $(jq -c '.seen.landed' "$OUT/retry-page.json" 2> /dev/null)"
}
