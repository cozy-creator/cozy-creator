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
  local project _
  [ -z "${PRE_PAGE:-}" ] || kill "$PRE_PAGE" 2> /dev/null || true
  PRE_PAGE=''
  [ -z "${PRE_SLOWED:-}" ] || sudo tc qdisc del dev lo root 2> /dev/null || check "prefetch: the store's link wasn't restored"
  PRE_SLOWED=''
  # Its projects closed, as nothing shows them now: what the run keeps of them is read once their files stand still.
  for project in "$OUT/pre-switch" "$OUT/pre-ready" "$OUT/pre-quoted" "$OUT/pre-yields" "$OUT/pre-load" "$OUT/pre-disk"; do
    for _ in $(seq 75); do [ -e "$project/project.db-shm" ] || break; sleep 0.2; done
  done
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
# as guessed, quoted nothing to download; its record says it needed none (`weights.needs`, cl-1102), and this machine fetches nothing
# and builds no environment for it.
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
# What its machine had of what it loads, and fetched for it (cl-1102): all of it there, nothing to wait for.
echo "prefetch ready: its run's weights: $(jq -c '.measurements.weights // "none said"' "$OUT/pre-made.json" 2> /dev/null)"
J '(.measurements.weights.needs // 0) == 0' "$OUT/pre-made.json" || check "prefetch ready: its run waited for $(run_get "$OUT" pre-made .measurements.weights.needs) bytes of weights on the machine made ready for it"
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

# prefetch_quoted (cl-1104): two boxes of one function on different checkpoints, each quoted by what this computer holds of its own.
# The cells before left e2e/pre-b here whole, and of e2e/pre-a its VAE alone (pre-b reads it too); this computer's runs of the tool
# read both checkpoints, a box of pre-b only pre-a's VAE. A project no page shows (nothing is guessed for it) renders a box of pre-b,
# then a box of pre-a, here: each one's quoted download is what then lands for it, within 5%: nothing for pre-b's, pre-a's transformer
# (384 MiB) for pre-a's. Before, a function kept one set of reads, its newest box's, and a box of pre-a was quoted that box's read of
# pre-a's VAE, all here: nothing to fetch.
prefetch_quoted() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): two boxes of one function on different checkpoints are each quoted what lands for them"
local P=$OUT/pre-quoted tool=e2e-prefetched/still box began job run quoted why
cozy --json new --unencrypted "$P" > "$OUT/pre-quoted-new.json" 2>&1 || { check "prefetch quoted: new: $(tail -c 300 "$OUT/pre-quoted-new.json")"; return 0; }
in_project "$P" --json machine use local > "$OUT/pre-quoted-use.json" 2>&1 || { check "prefetch quoted: machine use local: $(tail -c 300 "$OUT/pre-quoted-use.json")"; return 0; }
jq -n --arg t "$tool" '[{name: "on_b", tool: $t, kind: "image", fields: {instructions: "A still of e2e/pre-b, here whole.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}},
  {name: "on_a", tool: $t, kind: "image", fields: {instructions: "A still of e2e/pre-a, its transformer not here."}}]' | boxes_in "$P" || return 0
for box in on_b on_a; do
  began=$(date -u +%s%3N)
  coming_go "$box" '["local"]'
  coming_done "$box" 600
  job=$(status "$P" | jq -c --arg a "$box" '[.board.jobs[]? | select(.asset == $a)] | last // empty')
  run=$(jq -r '.run // empty' <<< "${job:-null}")
  [ -n "$run" ] || { check "prefetch quoted: $box has no run: ${job:-no job}"; return 0; }
  echo "$P" > "$OUT/pre-quoted-$box.at"
  run_show "$OUT" "pre-quoted-$box" "$run"
  quoted=$(run_get "$OUT" "pre-quoted-$box" .placed.fetch_bytes)
  echo "prefetch quoted: $box's run $run placed $(jq -c '.placed | {machine, fetch_bytes, prepare_ms}' "$OUT/pre-quoted-$box.json" 2> /dev/null); its weights $(jq -c '.measurements.weights // "none said"' "$OUT/pre-quoted-$box.json" 2> /dev/null)"
  pre_log local "$OUT/pre-quoted-$box.tensord.log"
  pre_judge "$OUT/pre-quoted-$box.tensord.log" "$began" "$(jq -n --argjson q "${quoted:-0}" --arg b "$box" '{quoted: $q, box: $b}')" <<'JUDGE' > "$OUT/pre-quoted-$box.failed"
print(json.dumps({"box": facts["box"], "quoted_mib": round(facts["quoted"] / MIB, 1), "landed_mib": round(landed / MIB, 1), "objects": len(weights)}), file=sys.stderr)
if abs(facts["quoted"] - landed) > max(landed, facts["quoted"]) * 0.05 + MIB:
    print(f"{facts['box']} was quoted {facts['quoted'] / MIB:.0f} MiB to fetch here, and {landed / MIB:.0f} MiB landed for it")
JUDGE
  while read -r why; do check "prefetch quoted: $why"; done < "$OUT/pre-quoted-$box.failed"
done
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
}

# prefetch_load (cl-1104): a guess goes where a render would go now as the machines' loads change, with no edit. A project of this
# computer's machine and stand-p shows a box of e2e/pre-b, which this computer holds whole and stand-p in part (prefetch_yields): its
# guess is here. Then this computer runs a 5-minute still, and an update of its machine waits for that run (a machine updating isn't
# free, cl-1073): a render would go to stand-p now, which has the rest of pre-b's transformer to fetch, and the guess moves there within
# 30 s and stand-p gets it ready. Before, a project was guessed again only on an edit, a machine arriving or its machine leaving.
prefetch_load() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): a guess moves when its machine's load changes, with no edit"
local P=$OUT/pre-load n='' row='' moved=1 updating='' _
if [ -z "${PRE_PAGE:-}" ]; then
  check "prefetch load: the cells before it left no page"
else
  pre_project "$P" local,stand-p \
    && jq -n --arg t e2e-prefetched/still '[{name: "loaded", tool: $t, kind: "image", fields: {instructions: "A still whose machine gets busy.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' | boxes_in "$P" \
    && pre_ready "$P" loaded local 180 \
    && n=$(run_submit "$OUT" pre-busy run "$here/project/waits.py/wait" seconds=300 --on local)
fi
if [ -n "$n" ]; then
  for _ in $(seq 240); do
    cozy --json machine show local > "$OUT/pre-load-local.json" 2> /dev/null || true
    J '[.status.runs[]? | select(.state == "running")] | length > 0' "$OUT/pre-load-local.json" && break
    sleep 0.5
  done
  (cozy --json machine update local --gpu-memory-limit off -y > "$OUT/pre-load-update.json" 2>&1; echo $? > "$OUT/pre-load-update.code") &
  updating=$!
  for _ in $(seq 60); do
    cozy --json machine show local > "$OUT/pre-load-local.json" 2> /dev/null || true
    J '(.status.update // "") | test("wait")' "$OUT/pre-load-local.json" && break
    sleep 1
  done
  if J '(.status.update // "") | test("wait")' "$OUT/pre-load-local.json"; then
    for _ in $(seq 60); do
      row=$(pre_row "$P" loaded)
      J '.machine == "stand-p"' <<< "${row:-null}" && { moved=0; break; }
      sleep 0.5
    done
    echo "prefetch load: 30 s after this computer's update waited for its 5-minute still, the box's guess is on $(jq -r '.machine // "no machine"' <<< "${row:-null}")"
    [ "$moved" = 0 ] || check "prefetch load: this computer's update waits for its run, and the box's guess is still on $(jq -r '.machine // "no machine"' <<< "${row:-null}") 30 s on: it wasn't made again as the load changed"
    [ "$moved" != 0 ] || pre_ready "$P" loaded stand-p 180 || true
  else
    check "prefetch load: this computer's update never waited for its run: $(jq -c '.status.update' "$OUT/pre-load-local.json" 2> /dev/null)"
  fi
  # The still ends, the update goes, and this computer's machine starts again on it.
  (cd "${RUN_PROJECT:-$PWD}" && cozy --json run cancel "$n") > "$OUT/pre-busy-cancel.json" 2>&1 || check "prefetch load: run cancel $n: $(tail -c 300 "$OUT/pre-busy-cancel.json")"
  for _ in $(seq 240); do kill -0 "$updating" 2> /dev/null || break; sleep 1; done
  [ "$(cat "$OUT/pre-load-update.code" 2> /dev/null)" = 0 ] || check "prefetch load: this computer's update: $(tail -c 300 "$OUT/pre-load-update.json")"
  for _ in $(seq 120); do
    cozy --json machine show local > "$OUT/pre-load-local.json" 2> /dev/null || true
    J '.state == "ready" and (.status.update // "") == ""' "$OUT/pre-load-local.json" && break
    sleep 1
  done
  J '.state == "ready"' "$OUT/pre-load-local.json" || check "prefetch load: this computer's machine isn't ready after its update: $(jq -c '{state, update: .status.update}' "$OUT/pre-load-local.json" 2> /dev/null)"
fi
timeout 60 "$COZY" --json machine end stand-p --force -y > "$OUT/stand-p-end.json" 2>&1 || true
}

# prefetch_disk (cl-1104): a guess on a short disk, and a guess's install. stand-q, a new stand-in, keeps its store on a 3 GiB disk, 1
# GiB of it TensorFS's reserve. A run there reads e2e/pre-ask (256 MiB: a call's, the oldest it holds), and its machine starts again so
# no executor holds it. A project of stand-q shows a box of e2e-slow-install, whose environment takes a wheel a server here sends at
# 2 MiB/s: while stand-q builds it for the guess, a run of a script that needs an environment of its own is sent there, and the
# guess's build gives the install line to it ("gave its turn"). That box goes; a box of e2e/pre-a gets stand-q ready (704 MiB). A
# file leaves 150 MiB free past the reserve. A box of e2e/pre-b, whose transformer (384 MiB) doesn't fit, is guessed there: nothing is
# evicted or written for it. `cozy model install e2e/pre-req` (256 MiB) makes room: what only a guess brought goes first, pre-a, though
# pre-ask is older. pre-a's box stops saying ready; once e2e/pre-req is uninstalled there is room again, and its guess fetches pre-a
# again, ready on stand-q.
prefetch_disk() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): on a short disk a guess evicts nothing, goes first, says so and comes again; its install gives way to a run's"
[ -n "${PRE_PAGE:-}" ] || { check "prefetch disk: the cells before it left no page"; pre_end; return 0; }
local P=$OUT/pre-disk name=stand-q img=$RUNNER_TEMP/pre-disk.img mnt=/mnt/pre-disk asked='' n row size avail reserve filler before after holder pid _
id "cozy-$name" > /dev/null 2>&1 || sudo useradd -m "cozy-$name" || { check "prefetch disk: $name: no user"; pre_end; return 0; }
truncate -s 3072M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" && sudo chown "cozy-$name:" "$mnt" \
  || { check "prefetch disk: $name's 3 GiB disk was not made"; pre_end; return 0; }
sudo -u "cozy-$name" -H bash -c 'mkdir -p ~/.cozy && printf "[machine]\nstore = \"%s\"\n" "$1" > ~/.cozy/config.toml' _ "$mnt" \
  || { check "prefetch disk: $name's store was not set"; pre_end; return 0; }
cozy --json machine show local > "$OUT/set-local.json" || { check "prefetch disk: machine show local"; pre_end; return 0; }
stand_in "$name" || { pre_end; return 0; }
# A call's read of e2e/pre-ask, then its machine anew: nothing holds it.
run_cell "$OUT" pre-oldest "$here/script/disk.py" --model weights=e2e/pre-ask@0.1.0 text=oldest --on "$name" \
  || { check "prefetch disk: a run reading e2e/pre-ask on $name: $(run_get "$OUT" pre-oldest .reason.message | head -c 300)"; pre_disk_end; return 0; }
holder=$(sudo find "/home/cozy-$name/.cozy/machine" -name agent.lock.holder 2> /dev/null | head -1)
pid=$([ -z "$holder" ] || sudo grep -o '"pid": *[0-9]*' "$holder" | grep -o '[0-9]*$' || true)
[ -n "$pid" ] || { check "prefetch disk: $name's machine has no lock holder"; pre_disk_end; return 0; }
sudo kill "$pid"
for _ in $(seq 60); do case $(ps -o stat= -p "$pid" 2> /dev/null || true) in '' | Z*) break ;; esac; sleep 1; done
stand_as "$name" --json machine show local > "$OUT/pre-disk-again.json" || { check "prefetch disk: $name didn't start again: $(tail -c 300 "$OUT/$name.err")"; pre_disk_end; return 0; }
for _ in $(seq 120); do
  pre_machine "$name" "$OUT/pre-disk-up.json"
  J '.state == "ready"' "$OUT/pre-disk-up.json" && break
  sleep 1
done
pre_project "$P" "$name" || { pre_disk_end; return 0; }
# A guess's environment that takes a while: the wheel it needs comes at 2 MiB/s.
python3 "$here/project/slowinstall/wheel.py" "$OUT/slow-wheel" $((2 << 20)) > "$OUT/slow-wheel.log" 2>&1 &
PRE_WHEEL=$!
jq -n '[{name: "slow", tool: "e2e-slow-install/still", kind: "image", fields: {instructions: "A still whose tool takes a while to install."}}]' | boxes_in "$P" || { pre_disk_end; return 0; }
# Its machine building the tool's environment for the guess: the wheel it needs asked of the server (a guess's install isn't in the
# activity its Status lists).
for _ in $(seq 240); do
  grep -q '"GET /e2e_slow_wheel' "$OUT/slow-wheel.log" 2> /dev/null && break
  J '.ready == true' <<< "$(pre_row "$P" slow)" && break
  sleep 0.5
done
if grep -q '"GET /e2e_slow_wheel' "$OUT/slow-wheel.log" 2> /dev/null; then
  asked=$(date -u +%s%3N)
  n=$(run_submit "$OUT" pre-line run "$here/project/waits.py/wait" seconds=0 --on "$name")
  [ -n "$n" ] && run_wait "$OUT" pre-line && [ "$(run_get "$OUT" pre-line .status)" = completed ] \
    || check "prefetch disk: the run sent while the guess built didn't complete: $(run_get "$OUT" pre-line .reason.message | head -c 300)"
  echo "prefetch disk: the slow wheel was asked for $(grep -c '"GET /e2e_slow_wheel' "$OUT/slow-wheel.log") times by then (once more is the guess building again after the run's install)"
else
  check "prefetch disk: $name never asked for e2e-slow-install's wheel for the box's guess: its row: $(pre_row "$P" slow)"
fi
if [ -n "$asked" ]; then
  pre_log "$name" "$OUT/pre-disk-line.tensord.log"
  pre_judge "$OUT/pre-disk-line.tensord.log" "$asked" '{}' <<'JUDGE' > "$OUT/pre-disk-line.failed"
# Its install line from the run's sending on: each turn taken, a guess's (order class 3) or a run's; a guess's build giving its turn up;
# a guess's build done. The run's turn comes as the guess gives way, a moment either side, never once its build is done.
turns, gave, done = [], [], []
for line in open(log, errors="replace"):
    when = at(line)
    if when < since:
        continue
    order = re.search(r"\border=(\d+)", line)
    if "install line: its turn" in line and order:
        turns.append((when, max(int(order[1]) - 2, 0) >> 62 == 3))
    if "gave its turn to a run's install" in line:
        gave.append(when)
    if "install line: done" in line and "cozy:prepare:" in line:
        done.append(when)
run_turn = next((when for when, guess in turns if not guess), None)
print(json.dumps({"turns": len(turns), "run_turn": run_turn, "gave": gave[:3], "guess_done": done[:3]}), file=sys.stderr)
if run_turn is None:
    print("the run's environment never took the install line after it was sent")
elif not any(when <= run_turn + 2000 for when in gave):
    if any(when <= run_turn for when in done):
        print("the run's install took the line only once the guess's build was done: a guess's build kept the install line from a run's")
    else:
        print("no guess gave its turn to the run's install, and none was building: the guess's build was over before the run was sent")
JUDGE
  while read -r why; do check "prefetch disk: $why"; done < "$OUT/pre-disk-line.failed"
fi
# Only the machine its render would go to got it: the project's one, stand-q; never this computer, which it doesn't prefer.
cozy --json machine show local > "$OUT/pre-disk-local.json" 2> /dev/null || true
! J '[.status.environments[]? | select(.package == "e2e-slow-install")] | length > 0' "$OUT/pre-disk-local.json" \
  || check "prefetch disk: this computer installed e2e-slow-install, though the box's project renders on $name alone"
project_api "$P" /assets/slow -X DELETE > "$OUT/pre-disk-slow-delete.json" 2>&1 || check "prefetch disk: the slow box wasn't deleted"
kill "$PRE_WHEEL" 2> /dev/null || true
jq -n --arg t e2e-prefetched/still '[{name: "kept", tool: $t, kind: "image", fields: {instructions: "A still stand-q gets ready on a short disk."}}]' | boxes_in "$P" || { pre_disk_end; return 0; }
pre_ready "$P" kept "$name" 300 || { pre_disk_end; return 0; }
# 150 MiB past the reserve.
read -r size avail < <(df -B1 --output=size,avail "$mnt" | tail -1)
reserve=$((size / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30))
filler=$((avail - reserve - (150 << 20)))
[ "$filler" -gt 0 ] && sudo fallocate -l "$filler" "$mnt/pre-disk-filler" || { check "prefetch disk: the filler ($filler bytes) was not made"; pre_disk_end; return 0; }
pre_machine "$name" "$OUT/pre-disk-full.json"
before=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
echo "prefetch disk: 150 MiB past the reserve: pre-ask $(pre_held "$OUT/pre-disk-full.json" e2e/pre-ask), pre-a $(pre_held "$OUT/pre-disk-full.json" e2e/pre-a)"
# A guess that doesn't fit takes no room.
jq -n --arg t e2e-prefetched/still '[{name: "bounded", tool: $t, kind: "image", fields: {instructions: "A still whose transformer does not fit.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' | boxes_in "$P" || { pre_disk_end; return 0; }
sleep 15
pre_machine "$name" "$OUT/pre-disk-bounded.json"
after=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
row=$(pre_row "$P" bounded)
echo "prefetch disk: 15 s after pre-b's box was guessed: its row ${row:-none}; used $before, then $after; pre-ask $(pre_held "$OUT/pre-disk-bounded.json" e2e/pre-ask), pre-a $(pre_held "$OUT/pre-disk-bounded.json" e2e/pre-a), pre-b $(pre_held "$OUT/pre-disk-bounded.json" e2e/pre-b)"
for model in e2e/pre-ask e2e/pre-a; do
  J --arg m "$model" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$OUT/pre-disk-bounded.json" \
    || check "prefetch disk: $model went from $name for a guess that doesn't fit: $(pre_held "$OUT/pre-disk-bounded.json" "$model")"
done
[ "$after" -le $((before + (16 << 20))) ] || check "prefetch disk: a guess that doesn't fit wrote $(((after - before) >> 20)) MiB on a disk 150 MiB past its reserve"
! J '.ready == true' <<< "${row:-null}" || check "prefetch disk: pre-b's box says ready on $name, which has no room for its transformer"
project_api "$P" /assets/bounded -X DELETE > "$OUT/pre-disk-delete.json" 2>&1 || check "prefetch disk: the bounded box wasn't deleted"
# A request makes room: what only a guess brought goes first.
n=$(run_submit "$OUT" pre-req model install e2e/pre-req --on "$name")
{ [ -n "$n" ] && run_wait "$OUT" pre-req && [ "$(run_get "$OUT" pre-req .status)" = completed ]; } \
  || { check "prefetch disk: model install e2e/pre-req on $name: $(run_get "$OUT" pre-req .reason.message | head -c 300)"; pre_disk_end; return 0; }
pre_machine "$name" "$OUT/pre-disk-made-room.json"
echo "prefetch disk: once e2e/pre-req came: pre-ask $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-ask), pre-a $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-a), pre-req $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-req)"
J '[.status.models[]? | select(.repository == "e2e/pre-ask") | .checkpoints[]?.complete] | any' "$OUT/pre-disk-made-room.json" \
  || check "prefetch disk: e2e/pre-ask, which a call read, went to make room while e2e/pre-a, which only a guess brought, stayed: $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-a)"
for _ in $(seq 40); do
  row=$(pre_row "$P" kept)
  J '.ready != true' <<< "${row:-null}" && break
  sleep 0.5
done
echo "prefetch disk: pre-a's box once its weights went: ${row:-no row}"
J '.ready != true' <<< "${row:-null}" || check "prefetch disk: pre-a's box still says ready on $name 20 s after what its guess brought went"
# Room again: its guess comes again.
stand_as "$name" --json model uninstall e2e/pre-req > "$OUT/pre-disk-uninstall.json" || check "prefetch disk: $name's model uninstall e2e/pre-req: $(tail -c 300 "$OUT/$name.err")"
pre_ready "$P" kept "$name" 180 || true
pre_machine "$name" "$OUT/pre-disk-again.json"
echo "prefetch disk: room again: pre-ask $(pre_held "$OUT/pre-disk-again.json" e2e/pre-ask), pre-a $(pre_held "$OUT/pre-disk-again.json" e2e/pre-a)"
J '[.status.models[]? | select(.repository == "e2e/pre-a") | .checkpoints[]?.complete] | any' "$OUT/pre-disk-again.json" \
  || check "prefetch disk: pre-a isn't whole on $name again once there was room: $(pre_held "$OUT/pre-disk-again.json" e2e/pre-a)"
pre_disk_end
}
# pre_disk_end: stand-q ended, its filler gone, the page closed.
pre_disk_end() {
  [ -z "${PRE_WHEEL:-}" ] || kill "$PRE_WHEEL" 2> /dev/null || true
  sudo rm -f /mnt/pre-disk/pre-disk-filler || true
  timeout 60 "$COZY" --json machine end stand-q --force -y > "$OUT/stand-q-end.json" 2>&1 || true
  pre_end
}
