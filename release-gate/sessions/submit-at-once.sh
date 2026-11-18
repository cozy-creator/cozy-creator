# submit-at-once (cl-1123, Paul 11-17: a render never waits on a prepare; a submit never hangs past its caller). PRO6-33 waited 92 s:
# its send moved hougetsu to the open RC's next build first, then its install waited behind an Orbit guess's. One rental of $SKU, this
# computer a dev home whose first-party packages run from the packages_refs checkout `tree` (as Paul's laptop runs them), and a project
# shown as an open page shows it, with a picture, an Orbit box turning it and a Qwen Image box. Two arms, on the session's TensorD:
#   install  Calcifer started again and the project shown; the moment the pod lists a guess, the Qwen box rendered, then the Orbit
#            box. The Qwen render's ask answers within 5 s and its first run is accepted within 5 s (the Orbit one's is said: its
#            first ships Orbit's local code); no run waits in the install line behind a guess's install (a guess a run waits on goes
#            at its place); the Orbit call's install uses the environment the Orbit guess builds and gets past it (#3777: two builds
#            of one broke it), then is canceled. TensorD answers throughout (cl-1202: 11.4 s of no answer while Qwen's import pages
#            were recorded, 44199443528): no gap over 3 s between the status reads each second, and every list put within 2 s.
#   rc       the pod back on the RC's builds, held by nothing, as hougetsu was; once the RC moves past them, the Qwen box rendered
#            at once: its run is accepted within 5 s, with no update of the pod before it. The follower moves the idle pod 30 s or
#            more after Calcifer reads the move; a render it beats to the pod is said, and the next move is waited for.
# params:
#   tree         the packages_refs checkout the packages run from (required: -f packages_refs=pk=rc/<x>, tree pk)
#   rc_wait_min  how long the rc arm waits for the RC to move (default 25)
#   seconds      what the cap must buy (default 3600)
#   arms         which arms run (default "install rc")
#   probe        threads: the pod's TensorD threads sampled through the install arm (tensord-threads.py, over SSH); gdb: also
#                their backtraces as its first import pages are recorded (a build that allows a ptracer). Default none.
. "$(dirname "$0")/lib.sh"
PICTURE=$REF/seed.jpg TREE=$(param tree '') RC_WAIT=$(param rc_wait_min 25) BEGAN=$(date +%s)
ARMS=$(param arms 'install rc') PROBE=$(param probe '')
[[ $RC_WAIT =~ ^[0-9]+$ ]] || fail "rc_wait_min $RC_WAIT"
[[ $PROBE =~ ^(|threads|gdb)$ ]] || fail "probe $PROBE"
[[ " $ARMS " == *" install "* ]] || fail "arms $ARMS: the install arm runs first"
if [ -n "$PROBE" ]; then
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
[ -f "$PICTURE" ] || fail "no $PICTURE"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of the head>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/orbit/pyproject.toml" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] ||
  fail "the packages run from a checkout: dispatch with -f packages_refs=<name>=<ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
say "a dev home on cozy $V, its packages from $TREE $(git -C "$SOURCE" rev-parse --short HEAD 2> /dev/null)"

run_page import "$PICTURE" --name photo > "$OUT/box-photo.json" 2>&1 || fail "the picture: $(tail -c 300 "$OUT/box-photo.json")"
run_page add turn --tool orbit/orbit --fields '{"picture": "<photo>", "subject": "lighthouse"}' > "$OUT/box-turn.json" 2>&1 ||
  fail "the Orbit box: $(tail -c 300 "$OUT/box-turn.json")"
qwen() { jq -nc --argjson seed "$1" '{instructions: "a red lighthouse on a cliff at dusk, waves below, photo", turbo: true,
  prompt_upsampling: false, megapixels: 1, aspect_ratio: "1:1", seed: $seed}'; }
run_page add pier --tool qwen-image-2/image --fields "$(qwen 7)" > "$OUT/box-pier.json" 2>&1 || fail "the Qwen box: $(tail -c 300 "$OUT/box-pier.json")"

quote "$(param seconds 3600)"
rent
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.status.phase // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '.rc.detail' "$OUT/show-rc.json")"

# The pod's Status each second, as Calcifer relays it: its software, how it stands to the RC, and each guess's readiness.
( while :; do
    printf '{"at_ms":%s,"show":%s}\n' "$(date +%s%3N)" "$(cozy machine show "$NAME" --json 2> /dev/null |
      jq -c '{rc: .rc, version: .status.version, ahead: [.status.ahead[]? | {id, state, stage: .readiness.stage}]}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
shown() {
  python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown-$1.txt" 2>&1 & SHOWN=$!; BG="$BG $SHOWN"
  for _ in $(seq 1 60); do grep -q '"held"' "$OUT/shown-$1.txt" && break; sleep 1; done
  grep -q '"held": \["' "$OUT/shown-$1.txt" || fail "the project wasn't shown: $(tail -c 300 "$OUT/shown-$1.txt")"
}
# ask <label> <box>: its render asked as the page asks it (POST /runs), not followed; how long the ask took, in ms.
ask() {
  local t0 t1
  t0=$(date +%s%3N)
  timeout 120 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" --queue "$2" > "$OUT/ask-$1.json" 2>&1 || fail "$1: the ask: $(tail -c 300 "$OUT/ask-$1.json")"
  t1=$(date +%s%3N)
  echo "$t0" > "$OUT/asked-$1"; echo $((t1 - t0)) > "$OUT/ask-ms-$1"
  say "$1: $2's render asked, answered in $((t1 - t0)) ms"
}
# first_run <label> <target pattern> [kind] [seconds]: the first run of the ask (of `kind`, any by default: a local install's run may
# come before the call's), once its machine accepted it: `number created accepted`.
first_run() {
  local since n
  since=$(cat "$OUT/asked-$1")
  for _ in $(seq "${4:-60}"); do
    (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 20) > "$OUT/runs-$1.json" 2> /dev/null
    n=$(jq -r --argjson s "$since" --arg t "$2" --arg k "${3:-}" '[.runs[]? | select(.created_at_ms >= $s - 1000 and (.target | test($t))
      and ($k == "" or .kind == $k))] | min_by(.created_at_ms) | select(.accepted_at_ms != null) |
      "\(.number) \(.created_at_ms) \(.accepted_at_ms)"' "$OUT/runs-$1.json" 2> /dev/null)
    [ -z "$n" ] || { echo "$n"; return 0; }
    sleep 1
  done
  return 1
}
# judge <label> <run> [said]: its ask within 5 s, and its run accepted within 5 s of being made (`said`: only said).
judge() {
  local label=$1 number created accepted asked_ms
  [ -n "$2" ] || return 1
  read -r number created accepted <<<"$2"
  asked_ms=$(cat "$OUT/ask-ms-$label")
  say "$label: run $number created $(date -u -d @$((created / 1000)) +%T), accepted $((accepted - created)) ms later; the ask answered in $asked_ms ms"
  [ $((accepted - created)) -le 5000 ] || [ -n "${3:-}" ] || { say "FAIL: $label: run $number was accepted $((accepted - created)) ms after it was created"; RC=1; }
  [ "$asked_ms" -le 5000 ] || { say "FAIL: $label: its ask took $asked_ms ms"; RC=1; }
}
collect() {
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
  cp "$RUN_PROJECT/.vault-calcifer/diagnostics/walk.log" "$OUT/walk.log" 2> /dev/null || true
}
# mine <label>: the pod on the session's TensorD, by an update by hand (which holds it there).
mine() {
  [ "$(cozy machine show "$NAME" --json | jq -r .status.version)" = "$V" ] && return 0
  step "tensord-$1" machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod refused the session's TensorD"
  [ "$(cozy machine show "$NAME" --json | jq -r .status.version)" = "$V" ] || fail "the pod doesn't run TensorD $V"
}

pod() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }

# install: Calcifer started again with the project shown; the renders asked the moment the pod lists a guess.
mine install
if [ -n "$PROBE" ]; then
  [ "$PROBE" != gdb ] || pod 'export DEBIAN_FRONTEND=noninteractive; apt-get -qq update && apt-get -qq install -y gdb > /dev/null; gdb --version | head -1' \
    >> "$OUT/probe.txt" || say "probe: no gdb on $NAME"
  pod "mkdir -p /root/cl1202 && echo $(base64 -w0 "$(dirname "$0")/tensord-threads.py") | base64 -d > /root/cl1202/threads.py &&
    (setsid nohup python3 /root/cl1202/threads.py 240 /root/cl1202/out $([ "$PROBE" = gdb ] && echo gdb) < /dev/null > /root/cl1202/sampler.log 2>&1 &
     echo \$! > /root/cl1202/pid)" ||
    fail "probe: the sampler didn't start on $NAME"
  say "probe: $NAME's TensorD threads sampled ($PROBE)"
fi
cozy restart > "$OUT/restart-install.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart-install.txt")"
shown install
A0=$(date +%s%3N)
say "install: Calcifer started again and the project shown; waiting for the pod to list a guess"
for t in $(seq 1 600); do
  cozy machine show "$NAME" --json 2> /dev/null | jq -e '[.status.ahead[]? | select(.id | startswith("cozy:prepare:"))] | length > 0' > /dev/null && break
  [ "$t" -lt 600 ] || { collect; fail "the pod listed no guess in 300 s"; }
  sleep 0.5
done
say "install: the pod lists $(cozy machine show "$NAME" --json | jq -c '[.status.ahead[]? | {id: .id[13:25], state, stage: .readiness.stage}]')"
ask qwen pier
ask orbit turn
judge qwen "$(first_run qwen 'qwen-image-2')" || { collect; fail "qwen: no run accepted within 60 s"; }
judge orbit "$(first_run orbit 'orbit' '' 120)" said || { collect; fail "orbit: no run accepted within 120 s"; }
# Their calls: the Qwen one to its end; the Orbit one until its install is behind it (its weights would take long), then canceled.
QWEN=$(first_run qwen 'qwen-image-2' call 900) || { collect; fail "qwen: no call accepted within 900 s"; }
ORBIT=$(first_run orbit 'orbit' call 900) || { collect; fail "orbit: no call accepted within 900 s"; }
for t in $(seq 1 600); do
  (cd "$RUN_PROJECT" && cozy --json run watch "${ORBIT%% *}" --once --full) > "$OUT/run-orbit.json" 2> /dev/null
  s=$(jq -r .status "$OUT/run-orbit.json" 2> /dev/null)
  [[ $s == failed || $s == canceled || $s == completed ]] && break
  jq -e '[.. | strings | select(test("describing local/orbit|choosing the model|downloading|starting the executor"))] | length > 0' "$OUT/run-orbit.json" > /dev/null 2>&1 && break
  sleep 2
done
s=$(jq -r .status "$OUT/run-orbit.json" 2> /dev/null)
if [[ $s == failed || $s == canceled ]]; then
  say "FAIL: orbit: run ${ORBIT%% *} $s: $(jq -r '.reason.message // empty' "$OUT/run-orbit.json" | cut -c1-300)"; RC=1
else
  say "orbit: run ${ORBIT%% *} past its install ($s): canceled"
  python3 "$RUN_PAGE" cancel "$RUN_PROJECT" turn > "$OUT/cancel-orbit.json" 2>&1 || say "orbit: the cancel: $(tail -c 200 "$OUT/cancel-orbit.json")"
fi
(cd "$RUN_PROJECT" && timeout 1200 cozy run watch "${QWEN%% *}" > "$OUT/watch-qwen.txt" 2>&1)
(cd "$RUN_PROJECT" && cozy --json run watch "${QWEN%% *}" --once --full) > "$OUT/run-qwen.json" 2>&1
A1=$(date +%s%3N)
[ "$(jq -r .status "$OUT/run-qwen.json" 2> /dev/null)" = completed ] || { say "FAIL: qwen: run ${QWEN%% *} $(jq -r '.status, .reason.message' "$OUT/run-qwen.json" 2> /dev/null | paste -sd' ')"; RC=1; }
collect
cp "$OUT/tensord.log" "$OUT/tensord-install.log"
if [ -n "$PROBE" ]; then
  pod 'kill "$(cat /root/cl1202/pid)" 2> /dev/null; tar czf - -C /root/cl1202 . | base64 -w0' | base64 -d > "$OUT/probe.tgz" &&
    mkdir -p "$OUT/probe" && tar xzf "$OUT/probe.tgz" -C "$OUT/probe" || say "probe: the samples weren't fetched"
  rm -f "$OUT/probe.tgz"  # published only as text, which the scrub reads
fi
# TensorD answered throughout the arm (cl-1202): the longest gap between the status reads each second (each `machine show` waits on
# the pod's Status), and the slowest list put, as Calcifer timed it.
python3 - "$OUT/status.ndjson" "$OUT/calcifer.log" "$NAME" "$A0" "$A1" <<'PY' > "$OUT/answered.json"
import json, re, sys
from datetime import datetime, timezone
status, calcifer, name, a0, a1 = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
ms = lambda line: datetime.fromisoformat(line[:26].rstrip("Z")).replace(tzinfo=timezone.utc).timestamp() * 1000
at = [t for t in (json.loads(line)["at_ms"] for line in open(status) if line.startswith("{")) if a0 <= t <= a1]
gap, since = max(((b - a, a) for a, b in zip(at, at[1:])), default=(0, a0))
puts = [(int(m[1]), line[11:23]) for line in open(calcifer, errors="replace")
        if (m := re.search(rf"lists put on {re.escape(name)} in (\d+) ms", line)) and line[:4].isdigit() and a0 <= ms(line) <= a1]
put, put_at = max(puts, default=(0, ""))
print(json.dumps({"reads": len(at), "gap_ms": gap, "gap_from": datetime.fromtimestamp(since / 1000, timezone.utc).strftime("%H:%M:%S.%f")[:-3],
                  "puts": len(puts), "put_ms": put, "put_at": put_at}))
PY
read -r GAP GAP_FROM PUT PUT_AT < <(jq -r '"\(.gap_ms) \(.gap_from) \(.put_ms) \(.put_at)"' "$OUT/answered.json")
say "install: TensorD's answers: $(jq -r .reads "$OUT/answered.json") status reads, longest gap ${GAP} ms (from $GAP_FROM); $(jq -r .puts "$OUT/answered.json") list puts, slowest ${PUT} ms (${PUT_AT:-none})"
[ "${GAP:-99999}" -le 3000 ] || { say "FAIL: install: no status read answered for ${GAP} ms from $GAP_FROM"; RC=1; }
[ "${PUT:-99999}" -le 2000 ] || { say "FAIL: install: a list put took ${PUT} ms (at $PUT_AT)"; RC=1; }
# Each run's place in the install line behind a guess's install, to its turn; a guess a run waits on is raised to the run's place.
python3 - "$OUT/tensord-install.log" <<'PY' | tee -a "$R" > "$OUT/line.json"
import json, re, sys
from datetime import datetime
at = lambda line: datetime.fromisoformat(line[:26].rstrip("Z")).timestamp()
waits, raised, gave, lines = {}, set(), [], []
for line in open(sys.argv[1], errors="replace"):
    if "install line:" not in line and "local install:" not in line:
        continue
    lines.append(line.strip()[:260])
    run = (re.search(r'run="?([^" ]+)"?', line) or [None, ""])[1]
    if "raised for a run" in line:
        raised.add(run)
    if "gives the line to" in line:
        gave.append(line.strip()[:200])
    if not run.startswith("run-"):
        continue
    behind = re.search(r"after (cozy:prepare:\S+)", line)
    if "in line to install" in line and behind and behind[1] not in raised:
        waits.setdefault(run, {"from": at(line), "behind": line.split("after ")[-1][:120]})
    elif "its turn" in line and run in waits and "s" not in waits[run]:
        waits[run]["s"] = round(at(line) - waits[run]["from"], 2)
print(json.dumps({"waits": [{"run": run, **{k: v for k, v in w.items() if k != "from"}} for run, w in waits.items()],
                  "raised": sorted(raised), "gave_the_line": gave, "line": lines[-60:]}, indent=1))
PY
say "install: guesses raised for a run: $(jq -c .raised "$OUT/line.json"); lines given to a run: $(jq '.gave_the_line | length' "$OUT/line.json")"
jq -r '.waits[] | select(.s == null or .s > 1) | "\(.run) waited \(.s // "unended") s behind \(.behind)"' "$OUT/line.json" | while read -r w; do say "FAIL: install: $w"; done
jq -e '[.waits[] | select(.s == null or .s > 1)] | length == 0' "$OUT/line.json" > /dev/null || RC=1
grep -aE "No such file or directory|environment bytecode" "$OUT/tensord-install.log" | head -5 | while read -r l; do say "FAIL: install: $l"; done
! grep -aqE "No such file or directory|environment bytecode" "$OUT/tensord-install.log" || RC=1
if [[ " $ARMS " != *" rc "* ]]; then
  kill $BG 2> /dev/null; BG=''
  [ "$RC" = 0 ] && say "PASS: the install arm: each render's ask answered and its run accepted within 5 s; TensorD answered throughout"
  exit "$RC"
fi

# rc: the pod back on the RC's builds, held by nothing; once the RC moves past them, the Qwen box rendered at once.
python3 "$RUN_PAGE" set "$RUN_PROJECT" pier --fields "$(qwen 11)" > "$OUT/set-pier.json" 2>&1 || fail "rc: the Qwen box's seed: $(tail -c 300 "$OUT/set-pier.json")"
step back machine update "$NAME" -y || fail "rc: the pod didn't go back to the RC's builds"
deadline=$(( $(date +%s) + RC_WAIT * 60 )) try=0 judged=''
while [ -z "$judged" ] && [ "$(date +%s)" -lt "$deadline" ]; do
  state=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.rc.state // empty')
  [ "$state" = behind ] || { sleep 1; continue; }
  try=$((try + 1))
  say "rc $try: the RC moved past the pod: $(cozy machine show "$NAME" --json | jq -r '"it runs \(.status.version); \(.rc.detail)"'); the Qwen box rendered"
  ask "rc$try" pier
  RCRUN=$(first_run "rc$try" 'qwen-image-2' '' 300) || { collect; fail "rc $try: no run accepted within 300 s"; }
  read -r number created accepted <<<"$RCRUN"
  (cd "$RUN_PROJECT" && timeout 900 cozy run watch "$number" > "$OUT/watch-rc$try.txt" 2>&1)
  (cd "$RUN_PROJECT" && cozy --json run watch "$number" --once --full) > "$OUT/run-rc$try.json" 2>&1
  collect
  cp "$OUT/tensord.log" "$OUT/tensord-rc$try.log"
  # Each restart of the pod on new software, against the run's making, acceptance and start.
  python3 - "$OUT/tensord-rc$try.log" "$created" "$accepted" "$(jq -r '.started_at_ms // 0' "$OUT/run-rc$try.json")" <<'PY' | tee -a "$R" > "$OUT/rc$try.json"
import json, sys
from datetime import datetime
created, accepted, started = (int(a) / 1000 for a in sys.argv[2:5])
at = lambda line: datetime.fromisoformat(line[:26].rstrip("Z")).timestamp()
moves = [(at(line), line.strip()[:160]) for line in open(sys.argv[1], errors="replace") if "restarting on cozy" in line and at(line) >= created - 120]
print(json.dumps({"restarts": [{"s_after_made": round(t - created, 1), "line": l} for t, l in moves],
                  "before_accept": [l for t, l in moves if t <= accepted],
                  "before_start": [l for t, l in moves if accepted < t <= (started or accepted)]}, indent=1))
PY
  if jq -e '.before_accept | length > 0' "$OUT/rc$try.json" > /dev/null; then
    say "rc $try: the follower moved the pod before the run reached it; the next move is waited for"
    continue
  fi
  judge "rc$try" "$RCRUN"
  [ "$(jq -r .status "$OUT/run-rc$try.json")" = completed ] || { say "FAIL: rc $try: run $number $(jq -r .status "$OUT/run-rc$try.json")"; RC=1; }
  jq -e '.before_start | length == 0' "$OUT/rc$try.json" > /dev/null || say "rc $try: the pod restarted between the run's acceptance and its start"
  judged=1
done
[ -n "$judged" ] || say "rc: the RC didn't move past the pod in $RC_WAIT min, or the follower beat each render: the rc arm isn't judged"
kill $BG 2> /dev/null; BG=''
collect
[ "$RC" = 0 ] && say "PASS: each render's ask answered and its run was accepted within 5 s; no run's install waited behind a guess's; TensorD answered throughout"
