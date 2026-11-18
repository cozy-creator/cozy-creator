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
# And a `waiting-<page>` box for each page, which that page's cell renders once it has loaded (cl-1263): one render per box at a time.
for page in "${PAGE_NAMES[@]}"; do printf 'waiting-%s:\n  %s/wait: {}\n' "$page" "$FIX" >> "$RUN_PROJECT/boxes.yaml"; done
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

# holds <label> <machine>: `holding` rendered on <machine> until it holds it. Sets HOLDING_RUN.
holds() {
  render "$1-holding" holding "$2"
  HOLDING_RUN=$(run_of hold) || fail "$1: holding's render made no run: $(tail -c 300 "$OUT/$1-holding.err")"
  follow "$1-holding" "$HOLDING_RUN"
  for _ in $(seq 600); do
    watched "$HOLDING_RUN" | jq -r '.progress.stage // empty' | grep -qi holding && break
    [ "$(watched "$HOLDING_RUN" | jq -r '.status // empty')" = failed ] && fail "$1: holding failed: $(watched "$HOLDING_RUN" | jq -c .reason)"
    sleep 1
  done
  say "$1: run $HOLDING_RUN holds $2 for ${HOLD}s"
}
# rows <page> <machine> <enforce|plumbing>: the page's cell renders its own `waiting-<page>` once its page has loaded, so what that run
# waits for reaches the page after it opened (cl-1263), and reads its box, its machine's tab and its queue. Its errors are sorted:
# the row checks (a row begins with its box's words; a row says what the run says now), candidate delivery (the harness counting
# requests the page itself canceled, the gate's own finding, said beside and judging nothing), and any other. Enforced (a rental's
# GPU wait, whose reason comes after the run's last lifecycle event, so a row read from the machine list's copy never has it): a page
# named red must fail its rows and nothing else, every other page pass them. Either way any other error fails the session.
rows() {
  local page=$1 machine=$2 mode=$3 errors="$OUT/page-$1.errors" rows delivery other
  pw "$page" judge WAIT_BOX="waiting-$page" WAIT_RENDER="waiting-$page" WAIT_DIR="$RUN_PROJECT" WAIT_MACHINE="$machine"
  jq -r '.. | objects | select(has("errors")) | .errors[]? | .message // empty | split("\n")[0]' "$OUT/web/report-$page.json" 2> /dev/null | sort -u > "$errors"
  rows=$(grep -cE 'begins with what its box says|says what the run says now' "$errors")
  delivery=$(grep -c 'candidate delivery errors' "$errors")
  other=$(grep -vcE 'begins with what its box says|says what the run says now|candidate delivery errors' "$errors")
  say "page $page: $rows row checks failed, $other other failures (exit $PW_CODE); candidate delivery errors: $([ "$delivery" = 0 ] && echo none || echo "counted, the harness's")"
  [ "$other" = 0 ] || { say "FAIL page $page: $(grep -vE 'begins with what its box says|says what the run says now|candidate delivery errors' "$errors" | head -2 | tr '\n' ' ')"; RC=1; }
  [ "$mode" = enforce ] || return 0
  if [ "$page" = red ]; then
    [ "$rows" -gt 0 ] && say "page red: its rows fail, as they must ($(grep -E 'begins with|says what the run' "$errors" | tr '\n' ' '))" \
      || { say "FAIL page red: its rows pass, so it proves nothing"; RC=1; }
  else
    [ "$rows" = 0 ] && say "page $page: its rows pass" || { say "FAIL page $page: its rows: $(grep -E 'begins with|says what the run' "$errors" | tr '\n' ' ')"; RC=1; }
  fi
}
# all_ended <label>: every run of the session's project ended, `holding` and each page's `waiting-<page>` (its hold and up to 10 min).
all_ended() {
  for _ in $(seq $((HOLD + 600))); do
    [ -z "$( (cd "$RUN_PROJECT" && cozy --json run ls --limit 50) 2> /dev/null | jq -r '(.runs? // [.]) | .[]? | select(.status as $s | ["completed", "failed", "canceled"] | index($s) | not) | .number' 2> /dev/null | head -1)" ] && break
    sleep 2
  done
  say "$1: the session's runs ended"
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
  PW_CODE=$code
  if [ "$want" = judge ]; then
    :
  elif [ "$want" = fail ]; then
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
  # `holding` alone holds this machine's CPU, and each page renders its own box behind it: the stage's plumbing, its rows read but
  # not enforced (a CPU wait's reason comes or not as TensorD's queue says it, pre-flight 44307867239).
  holds dry local
  for page in "${PAGE_NAMES[@]}"; do rows "$page" local plumbing; done
  all_ended dry
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
holds gpu "$NAME"
for page in "${PAGE_NAMES[@]}"; do rows "$page" "$NAME" enforce; done
# Still held once every page has looked: each page's run waited for the GPU throughout.
[ "$(watched "$HOLDING_RUN" | jq -r '.status // empty')" = in_progress ] || { say "FAIL: run $HOLDING_RUN let the GPU go before the pages had looked: hold longer"; RC=1; }
all_ended gpu
cozy machine logs "$NAME" --since 3h -n 0 > "$OUT/machine.log" 2>&1
{ echo "## gpu-wait on $NAME ($SKU)"; grep -E '^[0-9:]+ (gpu|page|FAIL|PASS)' "$R"; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$RC" = 0 ] && say "PASS: each page's rows read: red's fail, every other's pass"
exit "$RC"
