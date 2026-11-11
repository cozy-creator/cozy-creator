# render-placement (cl-774, Paul 11-05: "If the end user rented a GPU, presumably they want to use it" and "Place it on whatever machine
# will get the job done as fast as possible"): a render the page asks with its Run on at Automatically (e2e/page.py `--on auto`), on a
# runner with no GPU and one rental: the rental and this computer, this computer last. Weightless local code, so the walk's choice is all
# that's measured:
#   - idle, nothing measured anywhere: a tie, which the rental takes;
#   - a function that loads a model on a GPU (e2e's heavy.py) never goes to this computer, which has none, while the rental is there, and
#     the page doesn't ask (it asks `fit` only of a render on `local` alone);
#   - the rental busy with a slow take: a weightless still, measured on the rental, goes to this computer, idle, which ends it first;
#   - the heavy one, the rental still busy: it waits for the rental.
# One rental of $SKU (any GPU).
. "$(dirname "$0")/lib.sh"
WEB=$(cd "$(dirname "$0")/../../e2e/cli" && pwd)
for f in web/heavy.py project/draw.py project/shots.py; do [ -f "$WEB/$f" ] || fail "no e2e/cli/$f"; done
J() { jq -e "$@" > /dev/null; }
cat > "$OUT/placement.yaml" <<YAML
version: 4
assets:
  first:
    tool: $WEB/project/draw.py/draw
    instructions: a lighthouse at dusk
  heavy:
    tool: $WEB/web/heavy.py/heavy
    instructions: a lantern in fog
  long: {tool: $WEB/project/shots.py/slow, seconds: 150}
  second:
    tool: $WEB/project/draw.py/draw
    instructions: a kite over dunes
  heavy_again:
    tool: $WEB/web/heavy.py/heavy
    instructions: a lamp in rain
YAML
run_page yaml "$OUT/placement.yaml" > "$OUT/yaml.txt" 2>&1 || fail "project.yaml: $(tail -3 "$OUT/yaml.txt")"
# machine_of <function>: the machine the newest render of it ran on (not its package's install).
machine_of() { (cd "$RUN_PROJECT" && cozy --json run ls --limit 0) | jq -r --arg f "$1" '[.runs[] | select((.target // "") | test("^\\S+/" + $f + "$"))][0].machine // empty'; }
# rendered <step> <asset>: rendered as the page asks it at Automatically, to its end; fails on a warning that this computer would do it poorly.
rendered() {
  say "step $1: render $2 on what the page sends for Automatically"
  run_page render --on auto "$2" > "$OUT/$1.txt" 2>&1 || fail "$1: render $2: $(tail -c 500 "$OUT/$1.txt")"
  ! grep -qE 'This computer has no GPU|has no supported GPU' "$OUT/$1.txt" || fail "$1: with a rental held, this computer was warned of: $(grep -oE '[^"]*no (supported )?GPU[^"]*' "$OUT/$1.txt" | head -1)"
}
[ -z "$(cozy --json machine ls | jq -r '.machines[] | select(.kind == "local") | .status.gpus[0].name // empty')" ] || fail "this runner has a GPU: heavy wouldn't be done poorly here"

quote 1200
rent
say "the set: $(cozy --json machine ls | jq -c '[.machines[] | select(.state == "ready" or .state == "booting") | {name, kind}]')"

rendered idle first
sent=$(jq -c 'select(.on) | .on' "$OUT/idle.txt"); say "the page sent on: $sent"
J --arg m "$NAME" '. == [$m, "local"]' <<< "$sent" || fail "Automatically sent $sent, not the rental, then this computer"
on=$(machine_of draw); say "idle, nothing measured: first ran on $on"
[ "$on" = "$NAME" ] || fail "an idle tie went to ${on:-nothing}, not the rental $NAME"

rendered poor heavy
on=$(machine_of heavy); say "heavy ran on $on"
[ "$on" = "$NAME" ] || fail "a model this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"

# The rental busy: a long still there, then a weightless one at Automatically.
run_page render --on "$NAME" --queue long > "$OUT/long-queued.txt" 2>&1 || fail "long: $(tail -3 "$OUT/long-queued.txt")"
# slow_runs <jq status test>: how many of long's runs stand so.
slow_runs() { (cd "$RUN_PROJECT" && cozy --json run ls --limit 0) | jq -r --arg s "$1" '[.runs[] | select((.target // "") | test("^\\S+/slow$")) | select(.status | test($s))] | length'; }
until [ "$(slow_runs in_progress)" -gt 0 ]; do
  [ "$(slow_runs 'completed|failed|canceled')" = 0 ] || fail "long ended before it was seen running: $(run_page board | jq -c '[.jobs[]? | {asset, state, machine, why}]')"
  sleep 5
done
say "long is running on $NAME"
rendered busy second
on=$(machine_of draw); say "the rental busy: second ran on $on"
[ "$on" = local ] || fail "with the rental busy, a still this computer does as well went to ${on:-nothing}, not local"

rendered busy-poor heavy_again
on=$(machine_of heavy); say "the rental busy: heavy_again ran on $on"
[ "$on" = "$NAME" ] || fail "with the rental busy, a model this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"
say "PASS: a tie and what this computer would do poorly went to the rental; with it busy, a still went here"
