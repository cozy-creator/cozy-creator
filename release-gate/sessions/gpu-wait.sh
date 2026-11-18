# gpu-wait (cl-1197): a real wait for a GPU on one rental, and the page saying it. Paul read PRO6-61's 96 s wait for 2 GPUs as stuck:
# its face and machine tab said its stage, "Preparing, time left unknown". The pod's one GPU: the box `holding`
# (fixtures/gpu-wait/gpuwait.py's `hold`) takes it `hold` seconds; the box `waiting` (its `wait`, under way on the CPU, whose call needs
# the GPU: a run that only stands in line says "Queued", on every page), rendered meanwhile, waits for it,
# and Calcifer says why (`stage`). While it waits, e2e/cli/web/gpu-wait.spec.js's GPU branch reads its face, its machine's tab and its
# machine's queue, once for each of the `pages` pod-session built (CANDIDATE_DIST: the same wait seen by each), with each look in paper
# and dark. A page named `red` must fail, every other pass. Any other check that needs a run waiting for a GPU (the scheduler's,
# cl-1187) can follow `stage`. Without `rent`, $0: the quote, then the same on this runner's machine, which has no GPU, so nothing waits:
# the page reads `holding` as it runs, and nothing of a wait is asked of it. The session's cozy is an RC build (pod-session `cozy`: a
# dev-build run of rc/<x> or a branch of it: rc-dev leaves no cozy to download), and the rental runs its TensorD before anything runs
# on it. Dispatched from rc/<x> or a proof/* branch: release-gate deploys from no other. params:
#   hold     how long `holding` keeps the GPU, s (default 300: each page's reads and looks fit in it)
#   pages    {name: cozy-creator ref}: the pages pod-session builds and the cell is run with, in the order named (default: the cozy's own)
#   seconds  what the cap must buy (default 1500)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WEB=$ROOT/e2e/cli/web FIX=$HERE/fixtures/gpu-wait/gpuwait.py
for tool in jq node npm google-chrome; do command -v "$tool" > /dev/null || fail "no $tool"; done
HOLD=$(param hold 300)
[[ $HOLD =~ ^[0-9]+$ ]] && [ "$HOLD" -ge 60 ] || fail "hold $HOLD"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of rc/<x> or a branch of it>"
mapfile -t PAGE_NAMES < <(param pages '{}' | jq -r 'keys_unsorted[]')
for name in "${PAGE_NAMES[@]}"; do [ -f "${PAGES:-}/$name/index.html" ] || fail "page $name wasn't built (pod-session's pages step: \$PAGES/$name)"; done
[ "${#PAGE_NAMES[@]}" -gt 0 ] || PAGE_NAMES=(own)
PW=$OUT/playwright-test
npm install -s --prefix "$PW" @playwright/test@1.55.0 > "$OUT/npm.log" 2>&1 || fail "npm install @playwright/test: $(tail -c 300 "$OUT/npm.log")"
# The boxes, in the session's project: `holding` holds `hold` seconds, `waiting` waits for its GPU.
printf 'holding:\n  %s/hold:\n    seconds: %s\nwaiting:\n  %s/wait: {}\n' "$FIX" "$HOLD" "$FIX" > "$RUN_PROJECT/boxes.yaml"
(cd "$RUN_PROJECT" && cozy --json apply -f boxes.yaml) > "$OUT/apply.json" 2>&1 || fail "cozy apply: $(tail -c 300 "$OUT/apply.json")"
URL=$(cd "$RUN_PROJECT" && cozy --json open 2> "$OUT/open.err" | jq -r '.url // empty')
[ -n "$URL" ] || fail "the project's page: $(tail -c 300 "$OUT/open.err")"

# watched <run>: what `cozy run watch` says of a run once, as JSON.
watched() { (cd "$RUN_PROJECT" && cozy --json run watch "$1" --once) 2> /dev/null | grep '^{' | tail -1; }
# render <label> <box> <machine>: its render followed in the background (stopping the follower never cancels it), its events in
# $OUT/<label>.ndjson.
render() { (cd "$RUN_PROJECT" && exec cozy --json render "$2" --on "$3" --new -y) > "$OUT/$1.ndjson" 2> "$OUT/$1.err" & }
# follow <label> <run>: every event of a run as it goes (its stages, its waits, what it waited on), in the background: what each phase its
# face could say came from, warm renders' included.
follow() { (cd "$RUN_PROJECT" && exec cozy --json run watch "$2") > "$OUT/$1-watch.ndjson" 2>&1 & }
# run_of <fn>: this project's newest run of gpuwait.py's <fn> that hasn't ended, once there is one (up to 2 min): its number. A
# render's own events name its run only once it's done.
run_of() {
  local n
  for _ in $(seq 240); do
    n=$( (cd "$RUN_PROJECT" && cozy --json run ls --limit 20) 2> /dev/null | jq -r --arg fn "/$1" \
      '(.runs? // [.]) | .[]? | select((.target // "") | endswith($fn)) | select(.status as $s | ["completed", "failed", "canceled"] | index($s) | not) | .number' 2> /dev/null | head -1)
    [ -n "$n" ] && { echo "$n"; return 0; }
    sleep 0.5
  done
  return 1
}

# stage <label> <machine> [gpu]: `holding` rendered on <machine> until it holds; then `waiting`, and, with `gpu`, until Calcifer says it
# waits. Sets HOLDING_RUN and WAITING_RUN (their numbers) and WAITING_SAID (its `waiting`).
stage() {
  local label=$1 machine=$2 gpu=${3:-} now
  render "$label-holding" holding "$machine"
  HOLDING_RUN=$(run_of hold) || fail "$label: holding's render made no run: $(tail -c 300 "$OUT/$label-holding.err")"
  follow "$label-holding" "$HOLDING_RUN"
  for _ in $(seq 600); do
    watched "$HOLDING_RUN" | jq -r '.progress.stage // empty' | grep -qi holding && break
    [ "$(watched "$HOLDING_RUN" | jq -r '.status // empty')" = failed ] && fail "$label: holding failed: $(watched "$HOLDING_RUN" | jq -c .reason)"
    sleep 1
  done
  local what=$machine
  [ -z "$gpu" ] || what="$machine's GPU"
  say "$label: run $HOLDING_RUN holds $what for ${HOLD}s"
  render "$label-waiting" waiting "$machine"
  WAITING_RUN=$(run_of wait) || fail "$label: waiting's render made no run: $(tail -c 300 "$OUT/$label-waiting.err")"
  follow "$label-waiting" "$WAITING_RUN"
  WAITING_SAID=''
  [ -n "$gpu" ] || return 0
  for _ in $(seq 120); do
    now=$(watched "$WAITING_RUN")
    WAITING_SAID=$(jq -r '.waiting // empty' <<<"$now")
    [ -n "$WAITING_SAID" ] && break
    sleep 1
  done
  [ -n "$WAITING_SAID" ] || fail "$label: run $WAITING_RUN never waited for the GPU: $(jq -c '{status, machine_state, progress}' <<<"$now")"
  # Under way and waiting, not standing in line: a queued run's face says its place, which is no wait the pages differ on.
  [ -z "$(jq -r '.queue_position // empty' <<<"$now")" ] || fail "$label: run $WAITING_RUN only stands in line ($(jq -c '{queue_position, waiting, machine_state}' <<<"$now")): no run under way waits for the GPU"
  say "$label: run $WAITING_RUN waits: \"$WAITING_SAID\"; its stage \"$(jq -r '.progress.stage // "none"' <<<"$now")\""
}
# line <label>: a run of this computer's own, from a folder that is no project (`cozy run`, as a person runs a tool by hand), holds
# local's CPU, then `waiting`, this project's box, stands in its line behind it. TensorD names a holder outside any project in the reason
# of whatever waits for it to every reader, so the machine's own view of `waiting` says why, as Calcifer's does (cl-1263; a project's
# run it names only to its own caller, cl-1130). Sets ALONE_PID and WAITING_RUN.
line() {
  local label=$1 alone
  alone=$(mktemp -d)
  (cd "$alone" && exec cozy --json run "$FIX/hold" seconds:="$HOLD" --on local) > "$OUT/$label-alone.ndjson" 2> "$OUT/$label-alone.err" &
  ALONE_PID=$!
  for _ in $(seq 600); do
    grep -qi '"stage":"[^"]*holding' "$OUT/$label-alone.ndjson" && break
    kill -0 "$ALONE_PID" 2> /dev/null || fail "$label: the run outside any project ended: $(tail -c 300 "$OUT/$label-alone.err")"
    sleep 1
  done
  grep -qi '"stage":"[^"]*holding' "$OUT/$label-alone.ndjson" || fail "$label: the run outside any project never held local"
  say "$label: a run outside any project holds local for ${HOLD}s"
  render "$label-waiting" waiting local
  WAITING_RUN=$(run_of wait) || fail "$label: waiting's render made no run: $(tail -c 300 "$OUT/$label-waiting.err")"
  follow "$label-waiting" "$WAITING_RUN"
}
# ended <label>: both renders ended (holding's `hold` and up to 10 min more), completed.
ended() {
  local run status
  for run in "$HOLDING_RUN" "$WAITING_RUN"; do
    for _ in $(seq $((HOLD + 600))); do
      status=$(watched "$run" | jq -r '.status // empty')
      case $status in completed | failed | canceled) break ;; esac
      sleep 1
    done
    [ "$status" = completed ] || { say "FAIL $1: run $run $status: $(watched "$run" | jq -c .reason)"; RC=1; }
    # Its whole record, its stages and what it waited on: what each phase its face could say came from.
    (cd "$RUN_PROJECT" && cozy --json run watch "$run" --full) > "$OUT/$1-run-$run.ndjson" 2>&1
  done
  say "$1: runs $HOLDING_RUN and $WAITING_RUN ended"
}

# pw <page> <pass|fail> [VAR=value…]: the cell's GPU branch with that page, its report as web.sh says it, judged as it's to end.
pw() {
  local page=$1 want=$2 code dist=(); shift 2
  [ "$page" = own ] || dist=(CANDIDATE_DIST="$PAGES/$page")
  say "page $page"
  env "$@" "${dist[@]}" WAIT_PAGE="$page" WAIT_URL="$URL" HOLDER="$HOME/.cozy/calcifer/calcifer.lock.holder" COZY="$(command -v cozy)" \
    CHROME="$(command -v google-chrome)" OUT="$OUT" ARGV_LOG="$OUT/argv.log" NODE_PATH="$PW/node_modules" \
    node "$PW/node_modules/@playwright/test/cli.js" test -c "$WEB/gpu.config.js" -g 'gpu-wait: ' > "$OUT/page-$page.log" 2>&1
  code=$?
  cp "$OUT/web/report.json" "$OUT/web/report-$page.json" 2> /dev/null
  node "$WEB/report.js" "$OUT/web/report-$page.json" gpu-wait > "$OUT/page-$page.report" 2>&1
  sed 's/^/    /' "$OUT/page-$page.report" >> "$R"
  if [ "$want" = fail ]; then
    [ "$code" != 0 ] && say "page $page: fails, as it must ($(grep -m1 -E '^FAILED' "$OUT/page-$page.report" | cut -c1-200))" \
      || { say "FAIL page $page: passes, so it proves nothing"; RC=1; }
  else
    [ "$code" = 0 ] && say "page $page: passes" || { say "FAIL page $page: exit $code ($(grep -m1 -E '^FAILED' "$OUT/page-$page.report" | cut -c1-300))"; RC=1; }
  fi
}

RENTING=$RENT
RENT=true quote "$(param seconds 1500)"
if [ "$RENTING" != true ]; then
  say "pre-flight: nothing rented; this runner's machine, which has no GPU: the page reads holding as it runs"
  # This runner's machine on the session cozy's builds: a dev build follows no RC, so none is waited for.
  cozy machine update local -y > "$OUT/local-update.txt" 2>&1 || say "cozy machine update local: $(tail -c 300 "$OUT/local-update.txt")"
  line dry
  # Nothing waits for a GPU here. `waiting` stands in this machine's line behind the run outside any project, a run whose box and rows
  # must tell one story (cl-1263): each page reads it, and a page named red must fail.
  for page in "${PAGE_NAMES[@]}"; do pw "$page" "$([ "$page" = red ] && echo fail || echo pass)" WAIT_BOX=waiting WAIT_RUN="$WAITING_RUN" WAIT_MACHINE=local; done
  wait "$ALONE_PID" || { say "FAIL dry: the run outside any project: $(tail -c 300 "$OUT/dry-alone.err")"; RC=1; }
  for _ in $(seq 600); do
    case $(watched "$WAITING_RUN" | jq -r '.status // empty') in completed | failed | canceled) break ;; esac
    sleep 1
  done
  [ "$(watched "$WAITING_RUN" | jq -r '.status // empty')" = completed ] || { say "FAIL dry: run $WAITING_RUN didn't complete"; RC=1; }
  say "dry: the run outside any project and run $WAITING_RUN ended"
  exit "$RC"
fi
GUARD_ALSO=own guard
order
ready
# The pod runs the session cozy's own TensorD, as gpu-queue's does: a dev build follows no RC, so none is waited for (session 44234639273
# waited 15 min for `rc.state`, null on a dev build, and rented for nothing). The pod keeps its image's released Runtime.
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
[ "$(jq -r '.status.version // empty' "$OUT/show-rc.json")" = "$V" ] || fail "$NAME runs TensorD $(jq -r '.status.version // "?"' "$OUT/show-rc.json"), not the session's $V"
gpus=$(jq '.status.gpus // [] | length' "$OUT/show-rc.json")
[ "$gpus" = 1 ] || fail "$NAME has $gpus GPUs: a wait needs exactly one"
say "on the session's build: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
# Each tool once, its environment and model here, so the wait is the GPU's alone.
for f in hold wait; do step "warm-$f" run "$FIX/$f" seconds:=1 --on "$NAME" || fail "$f didn't run on $NAME"; done
stage gpu "$NAME" gpu
for page in "${PAGE_NAMES[@]}"; do
  pw "$page" "$([ "$page" = red ] && echo fail || echo pass)" WAIT_BOX=waiting WAIT_RUN="$WAITING_RUN" WAIT_MACHINE="$NAME" WAIT_SAID="$WAITING_SAID"
done
# Still waiting once every page has looked: what each saw was the wait, not what came after it.
[ -n "$(watched "$WAITING_RUN" | jq -r '.waiting // empty')" ] || { say "FAIL: run $WAITING_RUN stopped waiting before the pages had looked: hold longer"; RC=1; }
ended gpu
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## gpu-wait on $NAME ($SKU)"; grep -E '^[0-9:]+ (gpu|page|FAIL|PASS)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$RC" = 0 ] && say "PASS: the wait said on the face, the tab and the queue by every page but red"
exit "$RC"
