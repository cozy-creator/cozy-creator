# render-placement (cl-774, Paul 11-05: "If the end user rented a GPU, presumably they want to use it" and "Place it on whatever machine
# will get the job done as fast as possible"): a render the page asks with no machine green (e2e/page.py `--on auto`: none sent,
# Calcifer places it, cl-894), on a runner with no GPU and one rental: the rental and this computer, this computer last, stopped or
# not (it stops after 15 idle minutes and starts when used; its start counts in its quote). Weightless local code, so the walk's choice
# is all that's measured:
#   - idle, this computer stopped, nothing measured anywhere: the rental, sooner or tied;
#   - a function that loads a model on a GPU (e2e's heavy.py) never goes to this computer, which has none, while the rental is there, and
#     the page doesn't ask (it asks `fit` only of a render on `local` alone);
#   - the rental busy with a slow take it has timed once, this computer stopped again and holding the still's code (a warm render
#     here): the still starts this computer, which ends it first. A machine quotes from what it has timed: an untimed take would
#     look short, and code never installed here quotes its install (TensorD's INSTALL_MS, 2 min);
#   - the heavy one, the rental still busy: it waits for the rental;
#   - this computer's fit for heavy, which it ran slowly before the rental (`seconds`), says how long it takes here against the rental's
#     card (cl-897).
# One rental of $SKU (any GPU). This computer runs the still too: dispatched with `runtime_ref` (RUNTIME_WHEEL), its machine takes that Runtime
# first, as e2e's candidates go on, since a cozy built from an RC may speak a protocol no released Runtime does.
. "$(dirname "$0")/lib.sh"
WEB=$(cd "$(dirname "$0")/../../e2e/cli" && pwd)
for f in web/heavy.py project/draw.py project/shots.py; do [ -f "$WEB/$f" ] || fail "no e2e/cli/$f"; done
J() { jq -e "$@" > /dev/null; }
jq -n --arg web "$WEB" '[
  {name: "first", tool: "\($web)/project/draw.py/draw", kind: "image", fields: {instructions: "a lighthouse at dusk"}},
  {name: "heavy", tool: "\($web)/web/heavy.py/heavy", kind: "image", fields: {instructions: "a lantern in fog"}},
  {name: "long", tool: "\($web)/project/shots.py/slow", kind: "video", fields: {seconds: 60}},
  {name: "long_again", tool: "\($web)/project/shots.py/slow", kind: "video", fields: {seconds: 60}},
  {name: "warm", tool: "\($web)/project/draw.py/draw", kind: "image", fields: {instructions: "a buoy at noon"}},
  {name: "second", tool: "\($web)/project/draw.py/draw", kind: "image", fields: {instructions: "a kite over dunes"}},
  {name: "heavy_again", tool: "\($web)/web/heavy.py/heavy", kind: "image", fields: {instructions: "a lamp in rain"}},
  {name: "heavy_here", tool: "\($web)/web/heavy.py/heavy", kind: "image", fields: {instructions: "a lamp in the dark", seconds: 150}}
]' | run_page boxes > "$OUT/boxes.txt" 2>&1 || fail "the boxes: $(tail -3 "$OUT/boxes.txt")"
# machine_of <function>: the machine the newest render of it ran on (not its package's install).
machine_of() { (cd "$RUN_PROJECT" && cozy --json run ls --limit 0) | jq -r --arg f "$1" '[.runs[] | select((.target // "") | test("^\\S+/" + $f + "$"))][0].machine // empty'; }
# rendered <step> <asset>: rendered as the page asks it at Automatically, to its end; fails on a warning that this computer would do it poorly.
rendered() {
  say "step $1: render $2 on what the page sends for Automatically"
  run_page render --on auto "$2" > "$OUT/$1.txt" 2>&1 || fail "$1: render $2: $(tail -c 500 "$OUT/$1.txt")"
  ! grep -qE 'This computer has no GPU|has no supported GPU' "$OUT/$1.txt" || fail "$1: with a rental held, this computer was warned of: $(grep -oE '[^"]*no (supported )?GPU[^"]*' "$OUT/$1.txt" | head -1)"
}
[ -z "$(cozy --json machine ls | jq -r '.machines[] | select(.kind == "local") | .status.gpus[0].name // empty')" ] || fail "this runner has a GPU: heavy wouldn't be done poorly here"
if [ -n "${RUNTIME_WHEEL:-}" ]; then
  cozy --json machine update local --runtime "$RUNTIME_WHEEL" -y > "$OUT/local-runtime.json" 2>&1 || fail "this computer's machine refused $RUNTIME_WHEEL: $(tail -c 300 "$OUT/local-runtime.json")"
  say "this computer's machine runs Runtime $(cozy --json machine show local | jq -r '.status.runtime // empty')"
fi
# local_stopped: this computer's machine exits as its 15 idle minutes end it (SIGTERM, a clean exit), and Calcifer lists it stopped.
local_stopped() {
  local pid
  pid=$(jq -r '.pid // empty' "$HOME/.cozy/machine/var/lib/cozy/machine/agent.lock.holder" 2> /dev/null || true)
  [ -z "$pid" ] || kill -TERM "$pid" 2> /dev/null || true
  for _ in $(seq 60); do
    [ "$(cozy --json machine ls | jq -r '.machines[] | select(.kind == "local") | .state')" = stopped ] && { say "this computer's machine is stopped"; return; }
    sleep 1
  done
  fail "this computer's machine isn't listed stopped: $(cozy --json machine ls | jq -c '[.machines[] | select(.kind == "local") | {state}]')"
}
# placed_among <step>: the machines Calcifer placed its render among (the board's `on`), the rental then this computer.
placed_among() {
  local among; among=$(run_page board | jq -c '.on // []'); say "$1: Calcifer placed it among $among"
  # What Calcifer said as it placed it (a machine that didn't quote says why), for a step that fails.
  tail -n 4000 "$HOME/.cozy/calcifer/calcifer.log" > "$OUT/calcifer-$1.log" 2> /dev/null || true
  J --arg m "$NAME" '. == [$m, "local"]' <<< "$among" || fail "$1: with no machine chosen, Calcifer placed it among $among, not the rental, then this computer"
}

say "step here: heavy_here rendered on this computer alone, held 150 s"
run_page render --on local heavy_here > "$OUT/here.txt" 2>&1 || fail "here: render heavy_here: $(tail -c 500 "$OUT/here.txt")"
[ "$(machine_of heavy)" = local ] || fail "heavy_here ran on $(machine_of heavy), not local"

quote 1200
rent
say "the set: $(cozy --json machine ls | jq -c '[.machines[] | select(.state == "ready" or .state == "booting") | {name, kind}]')"

local_stopped
rendered idle first
placed_among idle
on=$(machine_of draw); say "idle, this computer stopped, nothing measured: first ran on $on"
[ "$on" = "$NAME" ] || fail "with this computer stopped, the idle rental, sooner or tied, lost first to ${on:-nothing}"

rendered poor heavy
on=$(machine_of heavy); say "heavy ran on $on"
[ "$on" = "$NAME" ] || fail "a model this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"

# What a render here alone asks first: its time here against the rental's card, by both their runs of heavy (cl-897). Before the busy
# steps, so their placement can't hide it.
run_page fit heavy > "$OUT/fit.json" 2> "$OUT/fit.err" || fail "fit: $(cat "$OUT/fit.err")"
line=$(jq -r '.tools[0].line // empty' "$OUT/fit.json"); say "fit: $line"
card=$(cozy --json machine ls | jq -r --arg n "$NAME" '.machines[] | select(.name == $n) | .status.gpus[0].name // empty' | sed -e 's/^NVIDIA //' -e 's/^GeForce //')
grep -qE "^This computer has no GPU.* It takes about [0-9.]+ (s|min) here vs about [0-9.]+ (s|min) on a rented ${card}\.$" <<< "$line" \
  || fail "fit for heavy didn't compare its time here with the rental's ${card:-card}: ${line:-$(cat "$OUT/fit.json")}"

# What each machine quotes is what it does: the still's code here, timed here, and the slow take timed once on the rental.
if ! timeout 420 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on local warm > "$OUT/warm.txt" 2>&1; then
  # What this computer's machine did with it, for the failure.
  (cd "$RUN_PROJECT" && cozy --json run ls --limit 5) > "$OUT/warm-runs.json" 2>&1 || true
  cozy --json machine show local > "$OUT/warm-local.json" 2>&1 || true
  cozy machine logs local --lines 400 > "$OUT/warm-tensord.log" 2>&1 || true
  tail -n 4000 "$HOME/.cozy/calcifer/calcifer.log" > "$OUT/calcifer-warm.log" 2> /dev/null || true
  fail "warm, here: not made in 7 min: $(jq -c '[.runs[]? | {number, target, machine, status, waiting, reason: .reason.message}]' "$OUT/warm-runs.json" 2> /dev/null | head -c 600)"
fi
run_page render --on "$NAME" long > "$OUT/long-timed.txt" 2>&1 || fail "long, timed on $NAME: $(tail -3 "$OUT/long-timed.txt")"
# The rental busy: another slow take there, of the function it has timed, then a weightless still at Automatically.
# slow_runs <jq status test>: how many of long's runs stand so.
slow_runs() { (cd "$RUN_PROJECT" && cozy --json run ls --limit 0) | jq -r --arg s "$1" '[.runs[] | select((.target // "") | test("^\\S+/slow$")) | select(.status | test($s))] | length'; }
ended=$(slow_runs 'completed|failed|canceled')
run_page render --on "$NAME" --queue long_again > "$OUT/long-queued.txt" 2>&1 || fail "long_again: $(tail -3 "$OUT/long-queued.txt")"
for _ in $(seq 60); do
  [ "$(slow_runs in_progress)" -gt 0 ] && break
  [ "$(slow_runs 'completed|failed|canceled')" = "$ended" ] || fail "long_again ended before it was seen running: $(run_page board | jq -c '[.jobs[]? | {asset, state, machine, why}]')"
  sleep 5
done
[ "$(slow_runs in_progress)" -gt 0 ] || fail "long_again isn't running on $NAME after 5 min: $(run_page board | jq -c '[.jobs[]? | {asset, state, machine, why}]')"
say "long_again is running on $NAME"
local_stopped
rendered busy second
placed_among busy
on=$(machine_of draw); say "the rental busy: second ran on $on"
[ "$on" = local ] || fail "with the rental busy, a still this computer does as well went to ${on:-nothing}, not local"

rendered busy-poor heavy_again
on=$(machine_of heavy); say "the rental busy: heavy_again ran on $on"
[ "$on" = "$NAME" ] || fail "with the rental busy, a model this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"
say "PASS: this computer stopped, the idle render and what this computer would do poorly went to the rental; with it busy, a still started this computer and ran here; its fit compares here with the rental"
