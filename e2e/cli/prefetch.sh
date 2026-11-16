# cl-1104 (Paul, 11-16): "we could design + build out a general 'pre fetch system'. So you know how google chrome, when you're typing
# into a browser, will pre-fetch the url you're likely to complete typing? So that when you push 'enter' and finish typing it appears
# instantly? something like that. So, in the webui, when a user attaches a tool, like the qwen image 2.1 tool, we could figure out what
# machine, if we got a request, that request would land on. On that machine, we can pre-emptively install packages + models, so that
# when we do get a request, we'll have those already ready." Of a guess that changes: "when I attach a qwen image 2.1 node, presumably
# we start downloading qwen image 2.1 model, but then I switch to another checkpoint, then cancel the parts of that download which are
# not needed, and start downloading the new checkpoint instead." And: "prefetched files should have lower download priority, or can
# be cancelled altogether if we guess wrong."
#
# Its tool is published (e2e-prefetched/still, e2e/isolated.sh's `pre`): a transformer and a VAE, e2e/pre-a's unless its box names
# e2e/pre-b's transformer, which shares e2e/pre-a's VAE. Its machines are this computer's and a stand-in (stand-p, another user's cozy
# on this runner, as machine_set's); the isolated Hub's store is slowed to 30 MB/s, as a pod's link is the bottleneck. A page's stream
# shows each project, as a project is guessed for while a page shows it.

# pre_page: a page's events stream, held open ($PRE_PAGE), as the page's own fetch asks it (cl-985); pre_show <project folder>: what
# it shows now; pre_end: the page gone, the link as it was.
pre_page() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  [ -z "${PRE_PAGE:-}" ] || return 0
  PRE_HEADERS=(-H "Host: $(jq -r .address "$lock")" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  curl -sSN "${PRE_HEADERS[@]}" -H 'Accept: text/event-stream' "http://$(jq -r .address "$lock")/v1/events" > "$OUT/pre-page.sse" 2>&1 & PRE_PAGE=$!
  PRE_STREAM=''
  for _ in $(seq 100); do PRE_STREAM=$(sse "$OUT/pre-page.sse" | jq -r 'select(.type == "hello") | .stream' | head -1) && [ -n "$PRE_STREAM" ] && break; sleep 0.1; done
  [ -n "$PRE_STREAM" ] || { check "prefetch: the page's stream said no hello: $(head -c 300 "$OUT/pre-page.sse")"; pre_end; return 1; }
}
pre_show() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder id
  id=$(status "$1" | jq -r '.id // empty')
  [ -n "$id" ] || { check "prefetch: $1 has no id: $(head -c 300 "$OUT/status.err")"; return 1; }
  curl -fsS -X PUT "${PRE_HEADERS[@]}" -H "Origin: http://$(jq -r .address "$lock")" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg id "$id" '{projects: [$id]}')" "http://$(jq -r .address "$lock")/v1/events/$PRE_STREAM" > "$OUT/pre-show.json" 2>&1 \
    || { check "prefetch: the page couldn't show $1: $(tail -c 300 "$OUT/pre-show.json")"; return 1; }
}
pre_end() {
  [ -z "${PRE_PAGE:-}" ] || kill "$PRE_PAGE" 2> /dev/null || true
  PRE_PAGE=''
  [ -z "${PRE_SLOWED:-}" ] || sudo tc qdisc del dev lo root 2> /dev/null || check "prefetch: the store's link wasn't restored"
  PRE_SLOWED=''
}
# pre_project <folder> <machines>: a plain project of those machines, shown by the page.
pre_project() {
  cozy --json new --unencrypted "$1" > "$OUT/pre-new.json" 2>&1 || { check "prefetch: new $1: $(tail -c 300 "$OUT/pre-new.json")"; return 1; }
  in_project "$1" --json machine use "$2" > "$OUT/pre-use.json" 2>&1 || { check "prefetch: machine use $2: $(tail -c 300 "$OUT/pre-use.json")"; return 1; }
  pre_show "$1"
}
# pre_row <project> <box>: its guess's row on the board, the newest that names it: what the machine a render of it would go to has
# ready for it. Nothing: no machine is getting ready for it.
pre_row() { status "$1" | jq -c --arg n "$2" '[.board.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null || true; }
# pre_ready <project> <box> <machine> <seconds>: its row says it's ready on that machine within that.
pre_ready() {
  local row='' _
  for _ in $(seq $(($4 * 2))); do
    row=$(pre_row "$1" "$2")
    J --arg m "$3" '.ready == true and .machine == $m' <<< "${row:-null}" && return 0
    sleep 0.5
  done
  check "prefetch: $2 isn't ready on $3 after $4 s: its row: ${row:-none}"
  return 1
}
# pre_machine <machine> <file>: what it last told this computer of itself, kept; pre_deadline <file>: its idle deadline there (0:
# none, a job is queued or running on it).
pre_machine() { cozy --json machine show "$1" > "$2" 2> /dev/null || true; }
pre_deadline() { jq -r '.status.idle_deadline_ms // 0' "$1" 2> /dev/null || echo 0; }
# pre_held <file> <repository>: what its machine holds of it: its bytes there, those no other model shares, and whether a
# checkpoint of it is whole.
pre_held() { jq -c --arg m "$2" '[.status.models[]? | select(.repository == $m) | {total_bytes, unique_bytes, complete: [.checkpoints[]?.complete]}]' "$1" 2> /dev/null || echo '[]'; }
# pre_wait <project> <box> <MiB>: its guess's row says its machine has fetched that much for it, within 2 min; the bytes it said.
pre_wait() {
  local done=0 _
  for _ in $(seq 400); do
    done=$(pre_row "$1" "$2" | jq -r '.done // 0' 2> /dev/null || echo 0)
    [ "${done:-0}" -ge $(($3 << 20)) ] && break
    sleep 0.2
  done
  echo "${done:-0}"
}
# pre_log <machine> <file>: its TensorD's log of the last hour.
pre_log() {
  case $1 in
    local) cozy machine logs local --lines 0 --since 1h > "$2" 2>&1 || check "prefetch: machine logs local failed" ;;
    # stand-p's by its own cozy, as plan_fetch reads stand-c.
    *) stand_as "$1" machine logs local --lines 0 --since 1h > "$2" || check "prefetch: $1's machine logs failed" ;;
  esac
}
# pre_judge <log> <from ms> <facts JSON>: what its machine's downloader and install line did from then on (`fetching`, `landed`,
# `fetch refused` and `install line` in its TensorD's log), judged by the Python on stdin: `weights`, its objects by digest, each
# with when it began, landed or was refused and at which order's class (0 a run's, 2 a run's guess, 3 a caller's guess); `landed`,
# the bytes of those that landed; `installs`, when an environment's build took the install line. Each line it prints is a failure.
pre_judge() {
  local judge
  judge=$(cat)
  python3 - "$1" "$2" "$3" "$judge" <<'PY' || true
import json, re, sys
from datetime import datetime
log, since, facts, judge = sys.argv[1], int(sys.argv[2]), json.loads(sys.argv[3]), sys.argv[4]
def at(line):
    try:
        return int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
objects, installs = {}, []
for line in open(log, errors="replace"):
    when = at(line)
    if when < since:
        continue
    found = re.search(r": (fetching|landed|fetch refused) .*object=([0-9a-f]{64})", line)
    if found:
        size, order = re.search(r"\bbytes=(\d+)", line), re.search(r"\bpriority=(\d+)", line)
        seen = objects.setdefault(found[2], {"fetching": [], "landed": [], "refused": [], "bytes": 0, "class": None})
        seen["fetching" if found[1] == "fetching" else "landed" if found[1] == "landed" else "refused"].append(when)
        if size:
            seen["bytes"] = int(size[1])
        # 1 is software's order (a package's files): no weights. 0 is what a started call reads now: a run's.
        if order and seen["class"] is None:
            seen["class"] = -1 if int(order[1]) == 1 else max(int(order[1]) - 2, 0) >> 62
    if "install line: its turn" in line:
        installs.append(when)
MIB = 1 << 20
weights = {sha: seen for sha, seen in objects.items() if seen["class"] != -1}
landed = sum(seen["bytes"] for seen in weights.values() if seen["landed"])
exec(judge)
PY
}

# prefetch_switch: a box takes the tool on a project of this computer's machine, idle since a run's end: its machine starts on
# e2e/pre-a (its 320 MiB VAE first, the smaller) and the tool's environment with no run sent. A second into the VAE its box names
# e2e/pre-b's transformer instead. What both read (the VAE) goes on, each object downloaded once and never begun again; e2e/pre-a's
# transformer, which only the old guess read, never comes (704 MiB land, not 1088); e2e/pre-b's does, and the box says it's ready here.
# All of it at a caller's guess's order (class 3, behind every run's needs and guesses), and no job: the machine's idle deadline is
# the one it had before the box took the tool, while its weights come and once they're here.
prefetch_switch() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "prefetch: its stand-in and its slowed link are a fresh GitHub runner's"; return 0; }
step "prefetch (cl-1104): a box's checkpoint switched mid-prefetch: what both read goes on, what only the old one read stops, and nothing of it is a job"
local P=$OUT/pre-switch tool=e2e-prefetched/still began switched came d0 d1 d2
PRE_PAGE='' PRE_SLOWED=''
slow_store prefetch || return 0
PRE_SLOWED=1
# This computer's machine up, and idle from this run's end.
run_cell "$OUT" pre-up minimax-h3-tools/echo text=up --on local || { check "prefetch: this computer's machine ran no echo: $(run_get "$OUT" pre-up .reason.message)"; pre_end; return 0; }
sleep 3
pre_machine local "$OUT/pre-switch-before.json"
d0=$(pre_deadline "$OUT/pre-switch-before.json")
[ "${d0:-0}" -gt 0 ] || { check "prefetch: this computer's machine, idle, says no idle deadline: $d0"; pre_end; return 0; }
pre_page || return 0
pre_project "$P" local || { pre_end; return 0; }
began=$(date -u +%s%3N)
jq -n --arg t "$tool" '[{name: "still", tool: $t, kind: "image", fields: {instructions: "A still, of the transformer its box names."}}]' | boxes_in "$P" || { pre_end; return 0; }
came=$(pre_wait "$P" still 16)
[ "$came" -ge $((16 << 20)) ] \
  || { check "prefetch: this computer's machine fetched nothing for the box's tool with no run sent: its row: $(pre_row "$P" still)"; pre_end; return 0; }
pre_machine local "$OUT/pre-switch-during.json"
d1=$(pre_deadline "$OUT/pre-switch-during.json")
# A status asked while its list is admitted says none for that moment: asked once more.
[ "$d1" = "$d0" ] || { sleep 1; pre_machine local "$OUT/pre-switch-during.json"; d1=$(pre_deadline "$OUT/pre-switch-during.json"); }
switched=$(date -u +%s%3N)
project_api "$P" /assets/still -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"models": {"model": {"ref": "e2e/pre-b@0.1.0"}}}}' > "$OUT/pre-switch-patch.json" 2>&1 \
  || check "prefetch: the box's model wasn't switched: $(tail -c 300 "$OUT/pre-switch-patch.json")"
echo "prefetch switch: its row said $((came >> 20)) MiB had come when its box named e2e/pre-b's transformer"
pre_ready "$P" still local 600 || { pre_end; return 0; }
pre_machine local "$OUT/pre-switch-after.json"
d2=$(pre_deadline "$OUT/pre-switch-after.json")
echo "prefetch switch: local's idle deadline $d0 before, $d1 while its weights came, $d2 once ready; e2e/pre-a here: $(pre_held "$OUT/pre-switch-after.json" e2e/pre-a); e2e/pre-b: $(pre_held "$OUT/pre-switch-after.json" e2e/pre-b)"
[ "$d1" = "$d0" ] || check "prefetch: this computer's idle deadline was $d1 while a guess's weights came, $d0 before: a guess is no job"
[ "$d2" = "$d0" ] || check "prefetch: this computer's idle deadline is $d2 once the guess is ready, $d0 before: a guess never starts the idle clock again"
J '[.status.models[]? | select(.repository == "e2e/pre-b") | (.checkpoints | any(.complete == true)) or .total_bytes >= (660 * 1048576)] | any' "$OUT/pre-switch-after.json" \
  || check "prefetch: e2e/pre-b's transformer and VAE (704 MiB) aren't here though its box says ready: $(pre_held "$OUT/pre-switch-after.json" e2e/pre-b)"
J '[.status.models[]? | select(.repository == "e2e/pre-a") | .checkpoints[]?.complete] | all(. != true)' "$OUT/pre-switch-after.json" \
  || check "prefetch: e2e/pre-a is whole here: its transformer, which only the old guess read, came all the same"
pre_log local "$OUT/pre-switch.tensord.log"
pre_judge "$OUT/pre-switch.tensord.log" "$began" "$(jq -n --argjson s "$switched" '{switched: $s}')" <<'JUDGE' > "$OUT/pre-switch.failed"
again = [sha[:12] for sha, seen in weights.items() if len(seen["fetching"]) > 1]
went_on = [sha for sha, seen in weights.items() if seen["fetching"] and seen["landed"] and seen["fetching"][0] <= facts["switched"] < seen["landed"][0] and not seen["refused"]]
runs = [sha[:12] for sha, seen in weights.items() if seen["class"] != 3]
print(json.dumps({"objects": len(weights), "landed_mib": landed // MIB, "went_on_through_the_switch": len(went_on), "refused": sum(1 for seen in weights.values() if seen["refused"]), "installs": len(installs)}), file=sys.stderr)
if again:
    print(f"{len(again)} objects were begun again ({again[:3]}): what both guesses read stopped and restarted")
if not went_on:
    print("no object in flight at the switch landed after it: what both guesses read didn't go on through it")
if landed > 832 * MIB:
    print(f"{landed // MIB} MiB landed, more than the VAE and one transformer (704) and what was in flight: e2e/pre-a's transformer came though no guess read it any more")
if landed < 640 * MIB:
    print(f"only {landed // MIB} MiB landed, less than the VAE and e2e/pre-b's transformer (704)")
if runs:
    print(f"{len(runs)} objects came at a run's order, not a caller's guess's (class 3): {runs[:3]}")
JUDGE
while read -r why; do check "prefetch switch: $why"; done < "$OUT/pre-switch.failed"
echo "$P" > "$OUT/pre-switch.at"
}

# prefetch_ready: a project of this computer's machine and stand-p, which holds nothing. Its box, of e2e/pre-b's transformer, is
# guessed onto this computer, where the last cell left its package and weights, though with all else equal a render goes to another
# machine before this computer: what a machine has counts. Its box says it's ready here, on its board and in the page beside its
# Render ("Ready on this computer", web/ready.js, which looks at it and presses Render); the render, naming no machine, goes here
# as guessed, quoted nothing to download, and this machine fetches nothing and builds no environment for it.
prefetch_ready() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): a box says its machine is ready, and Render goes there and downloads and installs nothing"
[ -n "${PRE_PAGE:-}" ] || { check "prefetch ready: the cell before it left no page"; return 0; }
local P=$OUT/pre-ready tool=e2e-prefetched/still pw=$OUT/playwright chrome code=0 began job run row
cozy --json machine show local > "$OUT/set-local.json" || { check "prefetch ready: machine show local"; return 0; }
stand_in stand-p || { pre_end; return 0; }
pre_project "$P" local,stand-p || { pre_end; return 0; }
jq -n --arg t "$tool" '[{name: "made", tool: $t, kind: "image", fields: {instructions: "A still this computer is ready for.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
pre_ready "$P" made local 180 || { pre_end; return 0; }
row=$(pre_row "$P" made)
began=$(date -u +%s%3N)
# The page, open on the project: its box says so beside Render, which it presses.
chrome=$(web_chrome)
if [ -n "$chrome" ] && [ -x "$chrome" ] && npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/pre-npm.log" 2>&1; then
  HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder CHROME=$chrome PLAYWRIGHT=$pw/node_modules/playwright-core SHOT=$OUT/pre-ready-page LAST_LOOK=$OUT/pre-ready-page \
    node "$here/web/ready.js" "$P" made 'this computer' > "$OUT/pre-ready-page.json" 2> "$OUT/pre-ready-page.err" || code=$?
  [ "$code" = 0 ] || check "prefetch ready: the page: $(jq -c '{failures, seen}' "$OUT/pre-ready-page.json" 2> /dev/null | head -c 1500) $(tail -c 400 "$OUT/pre-ready-page.err")"
else
  check "prefetch ready: no Google Chrome or playwright-core on this runner (${chrome:-no chrome}): $(tail -c 300 "$OUT/pre-npm.log" 2> /dev/null)"
fi
# Not pressed in the page, it is asked as the page asks it.
J '.seen.render_pressed == true' "$OUT/pre-ready-page.json" 2> /dev/null || coming_go made '[]'
coming_done made 600
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "made")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
[ -n "$run" ] && J '.state == "done"' <<< "$job" || { check "prefetch ready: its render didn't end done: ${job:-no job}"; pre_end; return 0; }
echo "$P" > "$OUT/pre-made.at"
run_show "$OUT" pre-made "$run"
echo "prefetch ready: guessed on $(jq -r .machine <<< "$row"); its render $run went to $(run_get "$OUT" pre-made .machine), placed $(jq -c '.placed | {machine, fetch_bytes, prepare_ms, warm}' "$OUT/pre-made.json" 2> /dev/null)"
J '.machine == "local"' "$OUT/pre-made.json" || check "prefetch ready: its render went to $(run_get "$OUT" pre-made .machine), not the machine its guess made ready (local)"
J '(.placed.fetch_bytes // 0) == 0' "$OUT/pre-made.json" || check "prefetch ready: its render was quoted $(run_get "$OUT" pre-made .placed.fetch_bytes) bytes to download on the machine made ready for it"
pre_log local "$OUT/pre-ready.tensord.log"
pre_judge "$OUT/pre-ready.tensord.log" "$began" '{}' <<'JUDGE' > "$OUT/pre-ready.failed"
print(json.dumps({"fetched_objects": len(weights), "installs": len(installs)}), file=sys.stderr)
if weights:
    print(f"its machine fetched {len(weights)} objects ({landed // MIB} MiB landed) from its render on: its critical-path download isn't nothing")
if installs:
    print(f"an environment was built {len(installs)} times from its render on: its package was installed again")
JUDGE
while read -r why; do check "prefetch ready: $why"; done < "$OUT/pre-ready.failed"
}

# prefetch_yields: a project of stand-p alone. Its box takes the tool: stand-p, which holds nothing, builds its environment and
# fetches e2e/pre-a (704 MiB). While that comes a request is sent there, `cozy model install e2e/pre-ask` (256 MiB): its weights
# come at once and alone, none of the guess's objects begun or landed while they do, in about what the link takes them (8.5 s);
# then the guess goes on and the box says ready on stand-p. A second box, of e2e/pre-b's transformer, is deleted while that comes:
# its row names no box, its download ends, and what had come of it stays as it was.
prefetch_yields() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): a request sent mid-prefetch isn't slowed, and a deleted box's prefetch stops"
[ -n "${PRE_PAGE:-}" ] || { check "prefetch yields: the cells before it left no page"; return 0; }
local P=$OUT/pre-yields tool=e2e-prefetched/still began asked came n first second gone=1 _
pre_project "$P" stand-p || { pre_end; return 0; }
began=$(date -u +%s%3N)
jq -n --arg t "$tool" '[{name: "first", tool: $t, kind: "image", fields: {instructions: "A still stand-p gets ready for."}}]' | boxes_in "$P" || { pre_end; return 0; }
came=$(pre_wait "$P" first 32)
[ "$came" -ge $((32 << 20)) ] || { check "prefetch yields: stand-p fetched nothing for the box's tool: its row: $(pre_row "$P" first)"; pre_end; return 0; }
asked=$(date -u +%s%3N)
n=$(run_submit "$OUT" pre-ask model install e2e/pre-ask --on stand-p)
[ -n "$n" ] || { check "prefetch yields: model install e2e/pre-ask was never a run: $(tail -c 300 "$OUT/pre-ask.err")"; pre_end; return 0; }
run_wait "$OUT" pre-ask || check "prefetch yields: the request's install didn't end"
[ "$(run_get "$OUT" pre-ask .status)" = completed ] || check "prefetch yields: the request's install $(run_get "$OUT" pre-ask .status): $(run_get "$OUT" pre-ask .reason.message | head -c 300)"
pre_ready "$P" first stand-p 600 || { pre_end; return 0; }
pre_log stand-p "$OUT/pre-yields.tensord.log"
pre_judge "$OUT/pre-yields.tensord.log" "$began" "$(jq -n --argjson a "$asked" '{asked: $a}')" <<'JUDGE' > "$OUT/pre-yields.failed"
asks = [seen for seen in weights.values() if seen["class"] == 0 and seen["fetching"] and seen["fetching"][0] >= facts["asked"]]
if not asks or not all(seen["landed"] for seen in asks):
    print(f"the request's weights didn't all come at a run's order: {len(asks)} objects began, {sum(1 for seen in asks if seen['landed'])} landed")
else:
    t1, t2 = min(seen["fetching"][0] for seen in asks), max(seen["landed"][0] for seen in asks)
    asked_bytes = sum(seen["bytes"] for seen in asks)
    guesses = [seen for seen in weights.values() if seen["class"] == 3]
    began_meanwhile = [seen for seen in guesses if any(t1 < when < t2 for when in seen["fetching"])]
    landed_meanwhile = [seen for seen in guesses if any(t1 + 1500 < when < t2 for when in seen["landed"])]
    before = [seen for seen in guesses if seen["fetching"] and seen["fetching"][0] < t1]
    print(json.dumps({"request_mib": asked_bytes // MIB, "request_ms": t2 - t1, "mib_per_s": round(asked_bytes / MIB / max(t2 - t1, 1) * 1000, 1),
                      "guess_objects_before": len(before), "began_meanwhile": len(began_meanwhile), "landed_meanwhile": len(landed_meanwhile)}), file=sys.stderr)
    if not before:
        print("no object of the guess had begun when the request's weights did: it wasn't sent mid-prefetch")
    if began_meanwhile:
        print(f"{len(began_meanwhile)} of the guess's objects began while the request's weights came: a prefetch took the link from a request")
    if landed_meanwhile:
        print(f"{len(landed_meanwhile)} of the guess's objects landed while the request's weights came: a prefetch shared the link with a request")
    if asked_bytes < 200 * MIB:
        print(f"the request fetched only {asked_bytes // MIB} MiB at a run's order, of e2e/pre-ask's 256")
    if t2 - t1 > 20000:
        print(f"the request's {asked_bytes // MIB} MiB took {(t2 - t1) / 1000:.1f} s, and the 30 MB/s link alone takes them about 8.5 s")
JUDGE
while read -r why; do check "prefetch yields: $why"; done < "$OUT/pre-yields.failed"

# A second box, deleted while its transformer comes.
jq -n --arg t "$tool" '[{name: "gone", tool: $t, kind: "image", fields: {instructions: "A still never rendered.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
# Its row counts the VAE, here already (320 MiB), with what comes: 32 MiB of its transformer in.
came=$(pre_wait "$P" gone 352)
[ "$came" -ge $((352 << 20)) ] || { check "prefetch yields: stand-p fetched nothing of e2e/pre-b's transformer for the second box: its row: $(pre_row "$P" gone)"; pre_end; return 0; }
project_api "$P" /assets/gone -X DELETE > "$OUT/pre-yields-delete.json" 2>&1 || check "prefetch yields: the second box wasn't deleted: $(tail -c 300 "$OUT/pre-yields-delete.json")"
for _ in $(seq 100); do
  [ -z "$(pre_row "$P" gone)" ] && { gone=0; break; }
  sleep 0.2
done
[ "$gone" = 0 ] || check "prefetch yields: 20 s after its box was deleted a row still names it: $(pre_row "$P" gone)"
# What was in flight as its run was canceled has let go.
sleep 3
pre_machine stand-p "$OUT/pre-yields-gone.json"
first=$(pre_held "$OUT/pre-yields-gone.json" e2e/pre-b)
sleep 5
pre_machine stand-p "$OUT/pre-yields-gone.json"
second=$(pre_held "$OUT/pre-yields-gone.json" e2e/pre-b)
echo "prefetch yields: its row said $((came >> 20)) MiB were there (320 of them its VAE) when its box was deleted; of e2e/pre-b stand-p holds $first, and 5 s on $second"
[ "$first" = "$second" ] || check "prefetch yields: e2e/pre-b went on coming after its box was deleted: $first, then $second"
J 'all(.complete | all(. != true))' <<< "$second" || check "prefetch yields: e2e/pre-b is whole on stand-p though its box was deleted as it came: $second"
[ -n "$(pre_row "$P" first)" ] || check "prefetch yields: the box that stayed lost its row with the one deleted"
timeout 60 "$COZY" --json machine end stand-p --force -y > "$OUT/stand-p-end.json" 2>&1 || true
pre_end
}
