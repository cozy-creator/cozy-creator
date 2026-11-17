# rentals-view (cl-1193): one list of the account's rentals, each saying what it does for whichever computer sent the work. Paul, 11-17:
# "ideally my webui (in the upper right ahnd ocrner can see all machines rented under my account (Even ones my current session is not
# using). Ideally. And I should be able to see their status and what they're doing."
# Two dispatches, each its own runner and Calcifer on the account:
#   role=worker (rent): the cheapest pod with SSH; its TensorD becomes this session's cozy; an SSH session stays open on it; a function
#     steps on it for `seconds` (fixtures/rentals-view/cl1193_steps.py); then a keepalive and the SSH session alone hold it for `hold_s`.
#   role=reader ($0): another computer of the account. It finds the worker's pod in its listing (a rental it didn't order, running
#     cl1193), and reads it as Paul's page does, while the run steps and again while only the SSH session and the keepalive hold it:
#     `cozy machine ls`/`show`, and the page in Chrome (the tabs top right, the machine's activity, Machines, the machine) at 1440x900
#     and 1920x1080 (fixtures/rentals-view/look.js).
# Params: role (worker|reader); seconds (worker, 360), hold_s (worker, 300); wait_s (reader, 1800).
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/rentals-view" && pwd) || fail "no fixtures"
check() { say "FAIL: $*"; echo "::error::$*"; RC=1; }

worker() {
  local run hold
  run=$(param seconds 360) hold=$(param hold_s 300)
  [ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
  RENT_ARGS=--ssh
  quote $((run + hold + 1200))
  rent
  step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
  (timeout $((run + hold + 900)) cozy machine ssh "$NAME" sleep $((run + hold + 600)) < /dev/null > "$OUT/ssh.txt" 2>&1) & BG="$BG $!"
  step steps run "$FIX/cl1193_steps.py" steps:=$((run / 2)) seconds:=2 --on "$NAME" || fail "the stepped run"
  step keepalive machine keepalive "$NAME" || fail "keepalive"
  say "held by the SSH session and a keepalive for ${hold}s"
  sleep "$hold"
  cozy machine show "$NAME" --json > "$OUT/worker-show.json" 2>&1
}

# shown <phase>: the worker's pod as this computer's listing says it, and the page's look at it.
shown() {
  cozy machine ls > "$OUT/$1-ls.txt" 2>&1
  cozy machine show "$W" > "$OUT/$1-show.txt" 2>&1
  cozy machine show "$W" --json > "$OUT/$1-show.json" 2>&1
  SHOT=$OUT/page PLAYWRIGHT=$PW/node_modules/playwright-core CHROME=$(command -v google-chrome) \
    node "$FIX/look.js" "$LINK" "$W" "$1" > "$OUT/$1-page.json" 2> "$OUT/$1-page.err" || check "$1: the page: $(jq -c .failures "$OUT/$1-page.json" 2> /dev/null) $(tail -c 300 "$OUT/$1-page.err")"
  jq -e '[.seen | to_entries[] | select(.key | endswith("machines")) | .value | test("rented by your other computers"; "i")] | any | not' "$OUT/$1-page.json" > /dev/null ||
    check "$1: Machines still lists other computers' rentals apart"
}

# held <jq filter>: waits until the worker's pod, as `machine show --json` says it, meets the filter.
held() {
  local until=$(($(date +%s) + 900))
  until cozy machine show "$W" --json 2> /dev/null | jq -e "$1" > /dev/null; do
    [ "$(date +%s)" -lt "$until" ] || return 1
    sleep 10
  done
}

reader() {
  local wait until
  wait=$(param wait_s 1800)
  for tool in jq node npm google-chrome; do command -v "$tool" > /dev/null || fail "no $tool"; done
  PW=$(mktemp -d)
  npm install -s --prefix "$PW" playwright-core@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install playwright-core: $(tail -c 300 "$OUT/npm.log")"
  cozy --json open > "$OUT/open.txt" 2>&1 || fail "cozy --json open: $(tail -c 300 "$OUT/open.txt")"
  LINK=$(jq -r .url "$OUT/open.txt")
  until=$(($(date +%s) + wait)) W=''
  while [ -z "$W" ]; do
    [ "$(date +%s)" -lt "$until" ] || fail "no rental another computer ordered ran cl1193 in ${wait}s"
    sleep 15
    # Its run names cl1193; a machine before cl-1193 names no computer for a run, so one that does is the worker's too.
    W=$(cozy machine ls --json 2> /dev/null | jq -r '[.machines[] | select(.state != "ended" and .ordered == false
      and any(.status.others[]?; ((.function // "") | test("cl1193")) or (.client // "") != ""))][0].name // empty')
  done
  say "the worker's pod, which another computer ordered: $W"

  held '[.status.others[]? | .step] | any(. > 1)' || check "its run never said a step"
  shown running
  jq -e '.ordered == false and .running >= 1 and .status.ssh_sessions >= 1
    and any(.status.others[]; ((.function // "") | test("cl1193")) and .running and (.client // "") != "" and .step > 0 and .steps > 0)' \
    "$OUT/running-show.json" > /dev/null || check "running: the listing doesn't say its run, its computer and step, and the SSH session: $(jq -c '{ordered, running, queued, status: .status | {others, ssh_sessions, kept_alive_ms}}' "$OUT/running-show.json")"
  jq -e '[.seen | to_entries[] | select(.key | endswith(" row")) | .value | test("From .*cl1193")] | (length > 0 and all)' "$OUT/running-page.json" > /dev/null ||
    check "running: the page's row doesn't say the run and who sent it"
  say "running: $(jq -c '.status.others' "$OUT/running-show.json")"

  held '((.status.others // []) | length == 0) and .status.ssh_sessions >= 1 and .status.kept_alive_ms != null' || check "it was never held by the SSH session and a keepalive alone"
  shown held
  jq -e '.ordered == false and .running == 0 and .status.ssh_sessions >= 1 and .status.kept_alive_ms != null' "$OUT/held-show.json" > /dev/null ||
    check "held: the listing doesn't say the SSH session and the keepalive: $(jq -c '{running, status: .status | {ssh_sessions, kept_alive_ms, idle_deadline_ms}}' "$OUT/held-show.json")"
  jq -e '[.seen | to_entries[] | select(.key | endswith(" row")) | .value | test("SSH session") and test("keepalive")] | (length > 0 and all)' "$OUT/held-page.json" > /dev/null ||
    check "held: the page's row doesn't say the SSH session and the keepalive"
  say "held: $(jq -c '.status | {ssh_sessions, kept_alive_ms, idle_deadline_ms}' "$OUT/held-show.json")"
}

case $(param role) in
  worker) worker ;;
  reader) reader ;;
  *) fail "role: worker or reader" ;;
esac
[ "$RC" != 0 ] || say "PASS"
exit "$RC"
