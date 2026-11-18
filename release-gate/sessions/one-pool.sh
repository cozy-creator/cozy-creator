# one-pool (cl-1224): the account's rentals are one pool, whichever of its computers ordered each. Paul, 11-18: "On your account,
# you can see all of your machines that you rented, and you can use all of them. There is no segregation on them. ... there is no
# 'this is mine, I rented this on this computer' sort of designation. Remove all such logic."
# Dispatches, each its own runner and Calcifer on the account:
#   role=worker (rent): the cheapest pod; its TensorD becomes this session's cozy; a cl1224 run steps on it, so the account's other
#     computers can name it; then keepalives hold it idle for `hold_s`, and it ends.
#   role=placer: another computer of the account, which rents nothing. It finds the worker's pod by that run, waits until it is idle
#     and the account's only live rental (so no other session's pod takes a run), then: a run naming no machine goes to it; `--on
#     any` goes to it; `--on <it>` completes there; a keepalive from here holds it; `cozy machine ls`/`show` and the page (the tabs,
#     Machines, the machine; 1440x900 and 1920x1080, fixtures/rentals-view/look.js) say it as any rental, never "other client" or
#     "rented by another computer".
# Before cl-1224, placement skipped a rental another client ordered: the run naming none went to local, and the words said whose it was.
# Params: role (worker|placer); hold_s (worker, 720); wait_s (placer, 1800).
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
  step mark run "$FIX/cl1224_steps.py" steps:=90 seconds:=2 --on "$NAME" || fail "the cl1224 run"
  say "held idle by keepalives for ${hold}s"
  until=$(($(date +%s) + hold))
  while [ "$(date +%s)" -lt "$until" ]; do
    cozy machine keepalive "$NAME" > /dev/null 2>&1
    sleep 120
  done
}

# go <name> <cozy run args…>: one run of the cl1224 function from this computer's project, followed to its end (10 min at most);
# its record in <name>.json. Prints the machine it ran on.
go() {
  local n=$1 run; shift
  (cd "$RUN_PROJECT" && timeout 600 cozy run "$FIX/cl1224_steps.py" steps:=2 seconds:=1 "$@") > "$OUT/$n.txt" 2>&1
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
  until=$(($(date +%s) + wait)) W=''
  while [ -z "$W" ]; do
    [ "$(date +%s)" -lt "$until" ] || fail "no rental of the account ran cl1224 in ${wait}s"
    sleep 15
    W=$(cozy machine ls --json 2> /dev/null | jq -r '[.machines[] | select(.kind == "rental" and .state != "ended"
      and any(.status.others[]?; (.function // "") | test("cl1224")))][0].name // empty')
  done
  say "the worker's pod: $W"
  until=$(($(date +%s) + 900))
  until cozy machine ls --json > "$OUT/alone.json" 2> /dev/null && jq -e --arg w "$W" '
      ([.machines[] | select(.kind == "rental" and .state != "ended" and .name != $w)] | length == 0)
      and any(.machines[]; .name == $w and .state == "ready" and .running == 0 and ((.status.others // []) | length == 0))' "$OUT/alone.json" > /dev/null; do
    [ "$(date +%s)" -lt "$until" ] || fail "$W never idle and alone: $(jq -c '[.machines[] | select(.kind == "rental" and .state != "ended")
      | {name, state, running, others: (.status.others // [] | length)}]' "$OUT/alone.json" 2> /dev/null)"
    sleep 15
  done
  say "$W is idle and the account's only live rental"

  m=$(go auto) || check "a run naming no machine didn't run"
  [ "$m" = "$W" ] || check "a run naming no machine went to ${m:-nothing}, not $W, the account's only rental"
  m=$(go any --on any) || check "--on any didn't run"
  [ "$m" = "$W" ] || check "--on any went to ${m:-nothing}, not $W, the account's only rental"
  m=$(go named --on "$W") || check "--on $W didn't run"
  [ "$m" = "$W" ] && [ "$(jq -r .status "$OUT/named.json")" = completed ] || check "--on $W: $(jq -c '{status, machine, reason}' "$OUT/named.json" 2> /dev/null)"
  cozy machine keepalive "$W" > "$OUT/keepalive.txt" 2>&1 || check "a keepalive from here: $(tail -c 300 "$OUT/keepalive.txt")"

  cozy machine ls > "$OUT/ls.txt" 2>&1
  cozy machine show "$W" > "$OUT/show.txt" 2>&1
  cozy machine show "$W" --json > "$OUT/show.json" 2>&1
  ! grep -i -E "$WORDS" "$OUT/ls.txt" "$OUT/show.txt" || check "the CLI says whose rental it is"
  ! jq -e 'has("ordered")' "$OUT/show.json" > /dev/null || check "machine show --json says ordered: $(jq -c .ordered "$OUT/show.json")"
  SHOT=$OUT/page PLAYWRIGHT=$PW/node_modules/playwright-core CHROME=$(command -v google-chrome) \
    node "$LOOK" "$LINK" "$W" idle > "$OUT/page.json" 2> "$OUT/page.err" || check "the page: $(jq -c .failures "$OUT/page.json" 2> /dev/null) $(tail -c 300 "$OUT/page.err")"
  jq -e --arg w "$WORDS" '[.seen[] | strings | test($w; "i")] | any | not' "$OUT/page.json" > /dev/null ||
    check "the page says whose rental it is: $(jq -c --arg w "$WORDS" '[.seen | to_entries[] | select(.value | strings | test($w; "i")) | .key]' "$OUT/page.json")"
}

case $(param role) in
  worker) worker ;;
  placer) placer ;;
  *) fail "role: worker or placer" ;;
esac
[ "$RC" != 0 ] || say "PASS"
exit "$RC"
