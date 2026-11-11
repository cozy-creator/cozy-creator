# run-no-on (tools.md, Paul 11-06: `cozy run` with no `--on` takes the render set): with a rental held, `cozy run <fixture>` with no `--on`
# lands on the rental, as a render does (render-placement.sh), and this computer, which loses a tie, takes nothing. e2e/cli/project.sh's
# run_no_on holds the case with no rental: this computer. Weightless local code, so where each run lands is all that's measured:
#   - idle, nothing measured anywhere: a tie, which the rental takes;
#   - a function that orchestrates a GPU model (e2e's heavy.py) on a runner with no GPU: the rental, with no warning of this computer.
# One rental of $SKU (any GPU). params:
#   traffic_gb  GB the provider bills (6)
. "$(dirname "$0")/lib.sh"
E2E=$(cd "$(dirname "$0")/../../e2e/cli" && pwd)
for f in project/tool.py web/heavy.py; do [ -f "$E2E/$f" ] || fail "no e2e/cli/$f"; done
# landed <step>: the machine the run its step names ran on.
landed() { cozy --json run watch "$(runno "$1")" --once | jq -r '.machine // empty'; }
# ran <step> <target> <field=value…>: `cozy run` with no --on, to its end; fails on a warning that this computer would do it poorly.
ran() {
  local n=$1; shift
  step "$n" run "$@" || fail "$n: cozy run $1: $(tail -3 "$OUT/$n.txt")"
  ! grep -qE 'has no supported GPU|This computer has no GPU' "$OUT/$n.txt" || fail "$n: with a rental held, cozy run warned of this computer: $(grep -E 'no (supported )?GPU' "$OUT/$n.txt")"
}
[ -z "$(cozy --json machine ls | jq -r '.machines[] | select(.kind == "local") | .status.gpus[0].name // empty')" ] || fail "this runner has a GPU: heavy wouldn't be done poorly here"

quote "$(param traffic_gb 6)" 900
rent
say "the set: $(cozy --json machine ls | jq -c '[.machines[] | select(.state == "ready" or .state == "booting") | {name, kind}]')"

ran idle "$E2E/project/tool.py/still" 'instructions=A harbor at dawn, seen from the breakwater.'
on=$(landed idle); say "idle, nothing measured: still ran on $on"
[ "$on" = "$NAME" ] || fail "with a rental held, cozy run with no --on went to ${on:-nothing}, not the rental $NAME"

ran poor "$E2E/web/heavy.py/heavy" 'prompt=A paper lantern drifting through fog.'
on=$(landed poor); say "heavy ran on $on"
[ "$on" = "$NAME" ] || fail "a function this computer has no GPU for went to ${on:-nothing}, not the rental $NAME"
say "PASS: cozy run with no --on went to the rental held, as a render does"
