# retry_elsewhere (cl-1085). Paul, 11-16: "when you shutdown a machine, and the job ends because of it, you should be given the chance to
# retry it (the toast notification tells you to retry it). you push the retry button and we'll try to place that run on a different machine
# then, or whatever we have available." Two boxes render on a stand-in machine (another user of this runner, as machine_set's), the project's
# one machine, each held there (waits.py's `held`), and it ends mid-run. Each run fails `machine_lost`, "stand-r shut down, so <run>
# stopped". In the page (web/retry.js) one box's toast, at the window's bottom left with no command, offers Retry, which renders it again
# on local; `cozy run retry` renders the other again on local; each on the seed it failed with.
retry_elsewhere() {
step "retry elsewhere (cl-1085): renders a machine's end cut short, retried on the machine left from the page's toast and cozy run retry"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "retry elsewhere: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/retry tool=$here/project/waits.py/held pw=$OUT/playwright chrome cut='' paged='' job pid code=0 name again
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

# The page open on the project as stand-r ends: its toasts, Retry, and paged rendered again.
HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder CHROME=$chrome PLAYWRIGHT=$pw/node_modules/playwright-core COZY=$COZY SHOT=$OUT/retry-page LAST_LOOK=$OUT/retry-page \
  node "$here/web/retry.js" "$P" stand-r "$paged" "$cut" > "$OUT/retry-page.json" 2> "$OUT/retry-page.err" || code=$?
[ "$code" = 0 ] || check "retry elsewhere: the page: $(jq -c '{failures, seen}' "$OUT/retry-page.json" 2> /dev/null | head -c 2000) $(tail -c 500 "$OUT/retry-page.err")"
timeout 60 "$COZY" --json machine end stand-r --force -y > /dev/null 2>&1 || true

# cut, from the CLI: its render ended saying why, then `cozy run retry` renders it again on local, on its seed.
for _ in $(seq 120); do kill -0 "$pid" 2> /dev/null || break; sleep 0.5; done
kill "$pid" 2> /dev/null && check "retry elsewhere: cozy render cut still followed it a minute after stand-r ended"
wait "$pid" 2> /dev/null || true
project_api "$P" "/runs/$cut?detail=full" > "$OUT/retry-cut-run.json" 2>&1
name=$(jq -r '.name // empty' "$OUT/retry-cut-run.json")
J --arg n "$name" '.status == "failed" and .reason.code == "machine_lost" and .reason.message == "stand-r shut down, so \($n) stopped" and (.reason.next | index("cozy run retry \($n)"))' "$OUT/retry-cut-run.json" \
  || check "retry elsewhere: cut's run didn't fail as its machine shut down: $(jq -c '{name, status, reason}' "$OUT/retry-cut-run.json")"
grep -q 'stand-r shut down' "$OUT/retry-cut.ndjson" "$OUT/retry-cut.err" || check "retry elsewhere: cozy render cut didn't say stand-r shut down: $(tail -c 300 "$OUT/retry-cut.err")"
row "$P" cut '.failure' | J --arg n "$name" '.code == "machine_lost" and .message == "cut: stand-r shut down, so \($n) stopped"' \
  || check "retry elsewhere: cut's failure: $(row "$P" cut '.failure')"
in_project "$P" --json run retry "$name" > "$OUT/retry-cli.ndjson" 2> "$OUT/retry-cli.err" \
  || check "retry elsewhere: cozy run retry $name: $(tail -c 400 "$OUT/retry-cli.err") $(tail -c 300 "$OUT/retry-cli.ndjson")"
again=$(jq -r 'select(.type? == "take" and .take.name == "cut") | .take.run // empty' "$OUT/retry-cli.ndjson" 2> /dev/null | tail -1)
project_api "$P" "/runs/${again:-0}?detail=full" > "$OUT/retry-again.json" 2>&1
J --slurpfile was "$OUT/retry-cut-run.json" '.machine == "local" and .status == "completed" and .request.seed == $was[0].request.seed' "$OUT/retry-again.json" \
  || check "retry elsewhere: cut wasn't rendered again on local on its seed: $(jq -c '{name, machine, status, seed: .request.seed}' "$OUT/retry-again.json") was $(jq -c '.request.seed' "$OUT/retry-cut-run.json")"
[ "$(made "$P" cut)" -ge 1 ] || check "retry elsewhere: cut has no take after cozy run retry"
echo "retry elsewhere: cut $name -> $(jq -r '.name' "$OUT/retry-again.json") on $(jq -r '.machine' "$OUT/retry-again.json"); the page: $(jq -c '.seen.again' "$OUT/retry-page.json" 2> /dev/null)"
}
