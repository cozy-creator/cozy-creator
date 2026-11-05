# render-placement (cl-774, Paul 11-05: "If the end user rented a GPU, presumably they want to use it" and "Place it on whatever machine
# will get the job done as fast as possible"): `cozy project render` with no `--on` on a runner with no GPU and one rental, as the page
# sends it: the rental and this computer, this computer last. Weightless local code, so the walk's choice is all that's measured:
#   - idle, nothing measured anywhere: a tie, which the rental takes;
#   - a function that loads a model on a GPU (e2e's heavy.py) never goes to this computer, which has none, while the rental is there, and
#     neither the CLI nor the page asks (`fit` asks only of a render on `local` alone);
#   - the rental busy with a slow take: a weightless still, measured on the rental, goes to this computer, idle, which ends it first;
#   - the heavy one, the rental still busy: it waits for the rental.
# One rental of $SKU (any GPU). params:
#   traffic_gb  GB the provider bills (6)
. "$(dirname "$0")/lib.sh"
WEB=$(cd "$(dirname "$0")/../../e2e/cli" && pwd)
for f in web/heavy.py project/draw.py project/shots.py; do [ -f "$WEB/$f" ] || fail "no e2e/cli/$f"; done
J() { jq -e "$@" > /dev/null; }
P=$OUT/placement
cozy project new "$P" > "$OUT/new.txt" 2>&1 || fail "project new: $(tail -3 "$OUT/new.txt")"
cat > "$P/project.yaml" <<YAML
version: 3
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
# machine_of <function>: the machine the newest render of it ran on (not its package's install).
machine_of() { cozy --json run ls -p "$P" --limit 0 | jq -r --arg f "$1" '[.runs[] | select((.target // "") | test("^\\S+/" + $f + "$"))][0].machine // empty'; }
# rendered <step> <asset>: `cozy project render <asset>` with no --on, to its end; fails on a warning that this computer would do it poorly.
rendered() {
  step "$1" project -p "$P" render "$2" || fail "$1: render $2: $(tail -3 "$OUT/$1.txt")"
  ! grep -q 'This computer has no GPU' "$OUT/$1.txt" || fail "$1: with a rental held, the CLI warned of this computer: $(grep 'no GPU' "$OUT/$1.txt")"
}
[ -z "$(cozy --json machine ls | jq -r '.machines[] | select(.kind == "local") | .status.gpus[0].name // empty')" ] || fail "this runner has a GPU: heavy wouldn't be done poorly here"

quote "$(param traffic_gb 6)" 1200
rent
say "the set: $(cozy --json machine ls | jq -c '[.machines[] | select(.state == "ready" or .state == "booting") | {name, kind}]')"

rendered idle first
on=$(machine_of draw); say "idle, nothing measured: first ran on $on"
[ "$on" = "$NAME" ] || fail "an idle tie went to ${on:-nothing}, not the rental $NAME"

rendered poor heavy
on=$(machine_of heavy); say "heavy ran on $on"
[ "$on" = "$NAME" ] || fail "a model this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"

# The rental busy: a long still there, then a weightless one with no --on.
step long-queued project -p "$P" render long --on "$NAME" --queue || fail "long: $(tail -3 "$OUT/long-queued.txt")"
# slow_runs <jq status test>: how many of long's runs stand so.
slow_runs() { cozy --json run ls -p "$P" --limit 0 | jq -r --arg s "$1" '[.runs[] | select((.target // "") | test("^\\S+/slow$")) | select(.status | test($s))] | length'; }
until [ "$(slow_runs in_progress)" -gt 0 ]; do
  [ "$(slow_runs 'completed|failed|canceled')" = 0 ] || fail "long ended before it was seen running: $(cozy project -p "$P" status 2>&1 | tail -3)"
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
