# fall_back (cl-920, Paul 11-12: "if those machines are ended then either try a different machine or tell me to rent a new one"): a
# project's chosen machine (stand-e, another user's cozy on this runner) ends. It leaves the project's machines at once; a render a page
# still sends to it goes where it finishes soonest and says why; a render and a `cozy run` naming none go there too. Never a bare refusal.
fall_back() {
step "fall back (cl-920): a project's chosen machine ended; its renders go where they finish soonest, and say so"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "fall back: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/fallback tool=$here/project/waits.py/wait job='' said
cozy --json machine show local > "$OUT/set-local.json" || { check "fall back: machine show local: $(head -c 300 "$OUT/set-local.json")"; return; }
stand_in stand-e || return
cozy --json new --unencrypted "$P" > "$OUT/fallback-new.json" 2>&1 || { check "fall back: new: $(tail -c 300 "$OUT/fallback-new.json")"; return; }
jq -n --arg tool "$tool" --arg r "$RANDOM" \
  '[{name: "after_end", tool: $tool, kind: "image", fields: {instructions: "Rendered once stand-e ended, \($r).", megapixels: 0.25, seconds: 1}}]' | boxes_in "$P" || return
in_project "$P" --json machine use stand-e > "$OUT/fallback-use.json" 2>&1 && J '.machines == ["stand-e"]' "$OUT/fallback-use.json" \
  || { check "fall back: machine use stand-e: $(tail -c 300 "$OUT/fallback-use.json")"; return; }
timeout 60 cozy --json machine end stand-e --force -y > "$OUT/fallback-end.json" 2>&1 || check "fall back: machine end stand-e: $(tail -c 300 "$OUT/fallback-end.json")"

# It left the project's machines.
in_project "$P" --json status > "$OUT/fallback-status.json" 2>&1
J '.project.machines == []' "$OUT/fallback-status.json" || check "fall back: stand-e ended and is still the project's: $(jq -c .project.machines "$OUT/fallback-status.json")"

# A page that hadn't heard sends the render to stand-e: it goes where it finishes soonest, saying stand-e ended.
project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: fallback-$RANDOM$RANDOM" \
  -d '{"names":["after_end"],"on":["stand-e"]}' > "$OUT/fallback-go.json" 2>&1 || check "fall back: render to stand-e: $(tail -c 300 "$OUT/fallback-go.json")"
for _ in $(seq 240); do
  job=$(status "$P" | jq -c '.board.jobs[]? | select(.asset == "after_end")' || true)
  J '.state == "done" or .state == "failed"' <<< "$job" && break
  sleep 0.5
done
J '.state == "done"' <<< "$job" || check "fall back: the render sent to stand-e isn't done: $job"
in_project "$P" --json run ls --limit 0 > "$OUT/fallback-rendered.json"
J '[.runs[] | select(.kind == "call")] | length == 1 and .[0].machine == "local" and .[0].status == "completed"' "$OUT/fallback-rendered.json" \
  || check "fall back: the render sent to stand-e didn't run on local: $(jq -c '[.runs[] | {number, kind, machine, status}]' "$OUT/fallback-rendered.json")"
said=$(status "$P" | jq -c '[.board.said[]? | .message]')
J 'any(.[]; test("stand-e ended") and test("finishes soonest"))' <<< "$said" || check "fall back: the render didn't say stand-e ended: $said"

# A `cozy run` naming none in it goes there too.
in_project "$P" --json run minimax-h3-tools/echo text=fallback > "$OUT/fallback-run.ndjson" 2> "$OUT/fallback-run.err" \
  || check "fall back: cozy run naming none: $(tail -c 300 "$OUT/fallback-run.err")"
in_project "$P" --json run ls --limit 0 > "$OUT/fallback-runs.json"
J '[.runs[] | select(.target | test("/echo"))] | .[0] | .machine == "local" and .status == "completed"' "$OUT/fallback-runs.json" \
  || check "fall back: cozy run naming none: $(jq -c '[.runs[] | {number, target, machine, status}]' "$OUT/fallback-runs.json")"
}
