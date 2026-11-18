# one-pool (cl-1224): the account's rentals are one pool, whichever of its computers ordered each. Paul, 11-18: "On your account,
# you can see all of your machines that you rented, and you can use all of them. There is no segregation on them. ... there is no
# 'this is mine, I rented this on this computer' sort of designation. Remove all such logic."
# Dispatches, each its own runner and Calcifer on the account:
#   role=worker (rent): the cheapest pod; its TensorD becomes this session's cozy, and with `runtime` it is held by hand on that Runtime
#     (`cozy machine update --runtime`, a hold kept in this computer's store); a cl1224 run steps on it, so the account's other
#     computers can name it; then keepalives hold it idle for `hold_s`, and it ends.
#   role=placer: another computer of the account, which rents nothing (its dispatch's cap only sets its deadline). It finds the
#     worker's pod by that run, waits until it is the account's only idle rental that's up (every other one runs or waits on work, as
#     its Status says, so a run naming none never lands on another session's idle pod), then: a run naming no machine goes to it
#     (of machines that finish it together, the one holding fewest runs); `--on <it>` completes there; a keepalive from here holds it; `cozy machine ls`/`show` and the page (the
#     tabs, Machines, the machine; 1440x900 and 768x1024, fixtures/rentals-view/look.js) say it as any rental, never "other client"
#     or "rented by another computer"; and a run naming no machine whose code needs a newer Runtime (fixtures/one-pool/cl1224_floor.py,
#     `cozy-runtime>=0.47.1`) never moves it off the Runtime the worker held it on: another computer's hold is invisible here, so
#     Cozy moves a rental another computer ordered only by hand (the reviewer's guard until the machine carries its hold, cl-1260).
# Before cl-1224, placement skipped a rental another client ordered: the run naming none went to local, and the words said whose it was.
# Params: role (worker|placer); hold_s (worker, 720); runtime (worker: the Runtime it holds the pod on, none by default); wait_s
# (placer, 1800); pod (placer: the worker's pod by name, for a placer sent after the worker's cl1224 run ended).
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/one-pool" && pwd) || fail "no fixtures"
LOOK=$(cd "$(dirname "$0")/fixtures/rentals-view" && pwd)/look.js
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }
trap 'code=$?; claim; (exit "$code"); finish' EXIT
WORDS='other client|rented by another computer'

worker() {
  local hold until
  hold=$(param hold_s 720)
  quote $((hold + 900))
  rent
  step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
  local runtime; runtime=$(param runtime)
  if [ -n "$runtime" ]; then
    step hold machine update "$NAME" --runtime "$runtime" -y || fail "hold on Runtime $runtime"
    [ "$(cozy machine show "$NAME" --json | jq -r '.status.runtime // empty')" = "$runtime" ] || fail "$NAME doesn't run Runtime $runtime"
    say "$NAME held by hand on Runtime $runtime: $(cozy machine show "$NAME" --json | jq -r '.held // "not held"')"
  fi
  step mark run "$FIX/cl1224_steps.py" steps:=90 seconds:=2 --on "$NAME" || fail "the cl1224 run"
  say "held idle by keepalives for ${hold}s"
  until=$(($(date +%s) + hold))
  while [ "$(date +%s)" -lt "$until" ]; do
    cozy machine keepalive "$NAME" > /dev/null 2>&1
    sleep 120
  done
}

# go <name> <fixture> <cozy run args…>: one run of a fixtures/one-pool function from this computer's project, followed to its end (10
# min at most); its record in <name>.json. Prints the machine it ran on.
go() {
  local n=$1 code=$2 run; shift 2
  (cd "$RUN_PROJECT" && timeout 600 cozy run "$FIX/$code" steps:=2 seconds:=1 "$@") > "$OUT/$n.txt" 2>&1
  run=$(runno "$n")
  [ -n "$run" ] || { say "$n: no run: $(tail -c 300 "$OUT/$n.txt")" >&2; return 1; }
  cozy run watch "$run" --once --json --full > "$OUT/$n.json" 2>&1
  say "$n: run $run $(jq -r '.status // "?"' "$OUT/$n.json") on $(jq -r '.machine // "?"' "$OUT/$n.json")" >&2
  jq -r '.machine // empty' "$OUT/$n.json"
}

placer() {
  local wait until m
  wait=$(param wait_s 1800)
  for tool in jq node npm google-chrome; do command -v "$tool" > /dev/null || fail "no $tool"; done
  PW=$(mktemp -d)
  npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
  (cd "$RUN_PROJECT" && cozy --json open) > "$OUT/open.txt" 2>&1 || fail "cozy --json open: $(tail -c 300 "$OUT/open.txt")"
  LINK=$(jq -r .url "$OUT/open.txt")
  until=$(($(date +%s) + wait)) W=$(param pod)
  while [ -z "$W" ]; do
    [ "$(date +%s)" -lt "$until" ] || fail "no rental of the account ran cl1224 in ${wait}s"
    sleep 15
    W=$(cozy machine ls --json 2> /dev/null | jq -r '[.machines[] | select(.kind == "rental" and .state != "ended"
      and any(.status.others[]?; (.function // "") | test("cl1224")))][0].name // empty')
  done
  say "the worker's pod: $W"
  until=$(($(date +%s) + 900))
  until cozy machine show "$W" --json 2> /dev/null | jq -e '.state == "ready" and (.running + .queued) == 0' > /dev/null; do
    [ "$(date +%s)" -lt "$until" ] || fail "$W never went idle"
    sleep 15
  done
  others_busy "$W"
  say "$W is the account's only idle rental that's up: $(jq -c '[.machines[] | select(.kind == "rental" and .state != "ended") | {name, state, running, queued}]' "$OUT/others.json")"

  m=$(go auto cl1224_steps.py) || check "a run naming no machine didn't run"
  [ "$m" = "$W" ] || check "a run naming no machine went to ${m:-nothing}, not $W, the account's only idle rental"
  m=$(go named cl1224_steps.py --on "$W") || check "--on $W didn't run"
  [ "$m" = "$W" ] && [ "$(jq -r .status "$OUT/named.json")" = completed ] || check "--on $W: $(jq -c '{status, machine, reason}' "$OUT/named.json" 2> /dev/null)"
  cozy machine keepalive "$W" > "$OUT/keepalive.txt" 2>&1 || check "a keepalive from here: $(tail -c 300 "$OUT/keepalive.txt")"

  cozy machine ls > "$OUT/ls.txt" 2>&1
  cozy machine show "$W" > "$OUT/show.txt" 2>&1
  cozy machine show "$W" --json > "$OUT/show.json" 2>&1
  ! grep -i -E "$WORDS" "$OUT/ls.txt" "$OUT/show.txt" || check "the CLI says whose rental it is"
  ! jq -e 'has("ordered")' "$OUT/show.json" > /dev/null || check "machine show --json says ordered: $(jq -c .ordered "$OUT/show.json")"
  SIZES=1440x900,768x1024 SHOT=$OUT/page PLAYWRIGHT=$PW/node_modules/playwright-core CHROME=$(command -v google-chrome) \
    node "$LOOK" "$LINK" "$W" idle > "$OUT/page.json" 2> "$OUT/page.err" || check "the page: $(jq -c .failures "$OUT/page.json" 2> /dev/null) $(tail -c 300 "$OUT/page.err")"
  jq -e --arg w "$WORDS" '[.seen[] | strings | test($w; "i")] | any | not' "$OUT/page.json" > /dev/null ||
    check "the page says whose rental it is: $(jq -c --arg w "$WORDS" '[.seen | to_entries[] | select(.value | strings | test($w; "i")) | .key]' "$OUT/page.json")"

  # A run naming no machine, whose code needs a newer Runtime than the one the worker held its pod on: it never moves the pod.
  local before after
  before=$(cozy machine show "$W" --json | jq -r '.status.runtime // empty')
  if [ "$before" = 0.47.1 ] || [ -z "$before" ]; then
    check "floor: $W runs Runtime ${before:-?}, which meets cl1224_floor's floor: the worker holds it on an older one (runtime=0.47.0)"
    return
  fi
  until cozy machine show "$W" --json 2> /dev/null | jq -e '.state == "ready" and (.running + .queued) == 0' > /dev/null; do sleep 10; done
  others_busy "$W"
  m=$(go floor cl1224_floor.py); say "floor: $(grep -m1 -iE 'error|refus|needs cozy-runtime|failed' "$OUT/floor.txt" | head -c 300)"
  [ -z "$m" ] || [ "$m" = "$W" ] || check "floor: the run went to $m, not $W, the account's only idle rental"
  sleep 30
  cozy machine show "$W" --json > "$OUT/floor-show.json" 2>&1
  after=$(jq -r '.status.runtime // empty' "$OUT/floor-show.json")
  say "floor: $W ran Runtime $before before, $after after; this computer's updates of it: $(jq -c '[.jobs[]? | select(.kind == "update") | {name, status, target}]' "$OUT/floor-show.json")"
  [ "$after" = "$before" ] && jq -e '[.jobs[]? | select(.kind == "update")] | length == 0' "$OUT/floor-show.json" > /dev/null ||
    check "a run naming no machine moved $W, which another computer holds on Runtime $before, to $after"
}

case $(param role) in
  worker) worker ;;
  placer) placer ;;
  *) fail "role: worker or placer" ;;
esac
[ "$RC" != 0 ] || say "PASS"
exit "$RC"
