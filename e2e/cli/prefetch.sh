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
  for project in "$OUT/pre-switch" "$OUT/pre-ready" "$OUT/pre-quoted" "$OUT/pre-yields" "$OUT/pre-load" "$OUT/pre-local" "$OUT/pre-disk" "$OUT/pre-retry" "$OUT/pre-order"; do
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
# with when it began, landed or was refused, at which order and its class (0 a run's, 2 a run's guess, 3 a caller's guess); `landed`,
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
            seen["order"] = int(order[1])
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

# A second box, deleted while its transformer comes. The store goes to 10 MB/s for it, so its transformer (384 MiB) is still
# coming when its drop lands: at 30 MB/s all six of its objects were in flight at once and had landed 10 ms before the drop came,
# leaving nothing to stop (e2e 44306642437, 44312762038). It goes back to 30 MB/s after.
local added deleted went=''
sudo tc qdisc del dev lo root 2> /dev/null; slow_store prefetch 80mbit || { pre_end; return 0; }
added=$(date -u +%s%3N)
jq -n --arg t "$tool" '[{name: "gone", tool: $t, kind: "image", fields: {instructions: "A still never rendered.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
# Its row counts the VAE, here already (320 MiB), with what comes: 32 MiB of its transformer in.
came=$(pre_wait "$P" gone 352)
[ "$came" -ge $((352 << 20)) ] || { check "prefetch yields: stand-p fetched nothing of e2e/pre-b's transformer for the second box: its row: $(pre_row "$P" gone)"; pre_end; return 0; }
deleted=$(date -u +%s%3N)
project_api "$P" /assets/gone -X DELETE > "$OUT/pre-yields-delete.json" 2>&1 || check "prefetch yields: the second box wasn't deleted: $(tail -c 300 "$OUT/pre-yields-delete.json")"
for _ in $(seq 100); do
  [ -z "$(pre_row "$P" gone)" ] && { gone=0; went=$(($(date -u +%s%3N) - deleted)); break; }
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
echo "prefetch yields: its row said $((came >> 20)) MiB were there (320 of them its VAE) $((deleted - added)) ms after its box was made, when it was deleted; its row went ${went:-never} ms after; of e2e/pre-b stand-p holds $first, and 5 s on $second"
[ "$first" = "$second" ] || check "prefetch yields: e2e/pre-b went on coming after its box was deleted: $first, then $second"
J 'all(.complete | all(. != true))' <<< "$second" || check "prefetch yields: e2e/pre-b is whole on stand-p though its box was deleted as it came: $second"
[ -n "$(pre_row "$P" first)" ] || check "prefetch yields: the box that stayed lost its row with the one deleted"

# A LoRA slot's choice swapped while its LoRA comes (cl-1281: a LoRA is a slot with a default a box overrides). `styled`'s slot
# default, e2e/pre-lora-a (128 MiB), comes at 10 MB/s; once its row says 16 MiB are there, the box names e2e/pre-lora-b. The old one
# stops where it was, and the new one comes whole before Render.
local at_swap swapped b_ms=''
jq -n '[{name: "styled", tool: "e2e-prefetched-lora/styled", kind: "image", fields: {instructions: "A still in a borrowed style."}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
came=$(pre_wait "$P" styled 16)
pre_machine stand-p "$OUT/pre-yields-lora.json"
at_swap=$(pre_held "$OUT/pre-yields-lora.json" e2e/pre-lora-a)
if [ "$came" -lt $((16 << 20)) ]; then
  check "prefetch yields: stand-p fetched nothing of e2e/pre-lora-a, styled's slot default: its row: $(pre_row "$P" styled)"
elif ! J 'all(.complete | all(. != true))' <<< "$at_swap"; then
  check "prefetch yields: e2e/pre-lora-a was whole before styled's swap ($((came >> 20)) MiB in its row), nothing left to stop: $at_swap"
else
  swapped=$(date -u +%s%3N)
  project_api "$P" /assets/styled -X PATCH -H 'content-type: application/json' \
    -d '{"fields": {"instructions": "A still in a borrowed style.", "models": {"lora": {"ref": "e2e/pre-lora-b@0.1.0"}}}}' \
    > "$OUT/pre-yields-swap.json" 2>&1 || check "prefetch yields: styled's lora wasn't swapped: $(tail -c 300 "$OUT/pre-yields-swap.json")"
  for _ in $(seq 240); do
    pre_machine stand-p "$OUT/pre-yields-lora.json"
    J 'length > 0 and all(.complete | any)' <<< "$(pre_held "$OUT/pre-yields-lora.json" e2e/pre-lora-b)" && { b_ms=$(($(date -u +%s%3N) - swapped)); break; }
    sleep 0.5
  done
  [ -n "$b_ms" ] || check "prefetch yields: e2e/pre-lora-b, styled's new LoRA, isn't whole on stand-p 2 min after the swap: $(pre_held "$OUT/pre-yields-lora.json" e2e/pre-lora-b)"
  pre_machine stand-p "$OUT/pre-yields-lora.json"
  first=$(pre_held "$OUT/pre-yields-lora.json" e2e/pre-lora-a)
  sleep 5
  pre_machine stand-p "$OUT/pre-yields-lora.json"
  second=$(pre_held "$OUT/pre-yields-lora.json" e2e/pre-lora-a)
  echo "prefetch yields: styled's slot swapped when its row said $((came >> 20)) MiB were there (e2e/pre-lora-a $at_swap); e2e/pre-lora-b whole ${b_ms:-never} ms after; e2e/pre-lora-a then $first, 5 s on $second"
  [ "$first" = "$second" ] || check "prefetch yields: e2e/pre-lora-a went on coming after styled's slot named e2e/pre-lora-b: $first, then $second"
  J 'all(.complete | all(. != true))' <<< "$second" || check "prefetch yields: e2e/pre-lora-a is whole on stand-p though styled's slot was swapped as it came: $second"
  pre_ready "$P" styled stand-p 60 || true
fi
sudo tc qdisc del dev lo root 2> /dev/null; slow_store prefetch || { pre_end; return 0; }
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

# prefetch_local (cl-1104): a box whose tool is local code, on a machine that never held it. Frozen-RC 44000183889 rented A for the
# user-flow, whose H3 runs from a checkout: its guess was never sent there (local code went only to a machine holding its source), and
# the box never said Ready. Here stand-l, a new stand-in, holds nothing; a project of it shows a box whose tool is e2e-prefetched as
# local code, its folder on this computer. The guess carries its source there as a run of it would, stand-l installs it and fetches
# e2e/pre-a, and the box says ready on stand-l. Its render goes there, quoted nothing to download, and fetches and builds nothing.
prefetch_local() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): local code is carried to a fresh machine, and its box says ready there"
[ -n "${PRE_PAGE:-}" ] || { check "prefetch local: the cells before it left no page"; return 0; }
local P=$OUT/pre-local name=stand-l began job run
cozy --json machine show local > "$OUT/set-local.json" || { check "prefetch local: machine show local"; return 0; }
stand_in "$name" || return 0
pre_project "$P" "$name" || { pre_local_end; return 0; }
jq -n --arg t "$here/project/prefetched/still" '[{name: "local_tool", tool: $t, kind: "image", fields: {instructions: "A still of local code, on a machine that never held it."}}]' \
  | boxes_in "$P" || { pre_local_end; return 0; }
pre_ready "$P" local_tool "$name" 300 || { pre_local_end; return 0; }
began=$(date -u +%s%3N)
coming_go local_tool "[\"$name\"]"
coming_done local_tool 600
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "local_tool")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
[ -n "$run" ] && J '.state == "done"' <<< "$job" || { check "prefetch local: its render didn't end done: ${job:-no job}"; pre_local_end; return 0; }
echo "$P" > "$OUT/pre-local-made.at"
run_show "$OUT" pre-local-made "$run"
echo "prefetch local: its render $run went to $(run_get "$OUT" pre-local-made .machine), placed $(jq -c '.placed | {machine, fetch_bytes, prepare_ms}' "$OUT/pre-local-made.json" 2> /dev/null); its weights $(jq -c '.measurements.weights // "none said"' "$OUT/pre-local-made.json" 2> /dev/null)"
J --arg m "$name" '.machine == $m' "$OUT/pre-local-made.json" || check "prefetch local: its render went to $(run_get "$OUT" pre-local-made .machine), not $name, which its guess made ready"
J '(.placed.fetch_bytes // 0) == 0' "$OUT/pre-local-made.json" || check "prefetch local: its render was quoted $(run_get "$OUT" pre-local-made .placed.fetch_bytes) bytes to download on $name"
pre_log "$name" "$OUT/pre-local.tensord.log"
pre_judge "$OUT/pre-local.tensord.log" "$began" '{}' <<'JUDGE' > "$OUT/pre-local.failed"
print(json.dumps({"fetched_objects": len(weights), "installs": len(installs)}), file=sys.stderr)
if weights:
    print(f"its machine fetched {len(weights)} objects ({landed // MIB} MiB landed) from its render on: the guess didn't have them ready")
if installs:
    print(f"an environment was built {len(installs)} times from its render on: the guess didn't install its local code")
JUDGE
while read -r why; do check "prefetch local: $why"; done < "$OUT/pre-local.failed"
pre_local_end
}
# pre_local_end: stand-l ended.
pre_local_end() { timeout 60 "$COZY" --json machine end stand-l --force -y > "$OUT/stand-l-end.json" 2>&1 || true; }

# prefetch_gpuless (cl-1279, Paul 11-18: "why are you trying to place jobs on a machine that cannot run them? … You can pre-fetch on
# GPU-less machines if it makes sense"): this computer has no GPU. A project of it alone, shown by the page, has two boxes of local
# code that each build a model: `unmeasured` (e2e/pre-req) has no time measured without a GPU; `measured` (e2e/pre-ask) has a CPU row
# in its benchmarks.json. `measured` is guessed onto this computer, made ready there, and its render runs there, quoted from its row.
# `unmeasured` is guessed nowhere; rendered, it waits with nothing sent, its box and the board saying this computer has no GPU and
# the rental, and none of e2e/pre-req lands here. Without cl-1279 `unmeasured` was sent here and ran on the CPU, and `measured` was
# never guessed (#3932's `not_simply`). cl-1286 (Paul, 11-18: a formula "hand-built per package based on measured times we store in
# the same benchmark"; #3983's review: a formula that reads no row must not make a GPU-less machine eligible): `authored` (e2e/pre-req)
# declares `ms` reading no row, and is treated as `unmeasured`; without cl-1286's fix it stood on its author's word and was sent
# here. `formula` (e2e/pre-ask) declares `ms` over its CPU row: guessed here, and its render quoted here from that row by its formula
# (`placed.formula_ms` its row's 4,000 ms times 2), as `measured`'s default is from its own (no `formula_ms`).
prefetch_gpuless() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch gpuless (cl-1279): a computer with no GPU takes a function that builds a model only on a time measured without one"
[ -n "${PRE_PAGE:-}" ] || { check "prefetch gpuless: the cells before it left no page"; return 0; }
local P=$OUT/pre-gpuless job run line='^This computer has no GPU, and nothing measured says this runs without one: render it on a new rental[.]$' _
pre_machine local "$OUT/gpuless-before.json"
J '(.status.gpus // []) | length == 0' "$OUT/gpuless-before.json" || { echo "prefetch gpuless: this computer has a GPU"; return 0; }
pre_project "$P" local || return 0
jq -n --arg s "$here/project/gpuless/still" --arg m "$here/project/gpuless/measured" '[
  {name: "unmeasured", tool: $s, kind: "image", fields: {instructions: "Nothing measured it without a GPU."}},
  {name: "measured", tool: $m, kind: "image", fields: {instructions: "Its CPU row times it."}},
  {name: "authored", tool: ($s | sub("/still$"; "/authored")), kind: "image", fields: {instructions: "Its author alone times it."}},
  {name: "formula", tool: ($m | sub("/measured$"; "/formula")), kind: "image", fields: {instructions: "Its formula over its CPU row times it."}}]' \
  | boxes_in "$P" || return 0
pre_ready "$P" measured local 300 || return 0
[ -z "$(pre_row "$P" unmeasured)" ] || check "prefetch gpuless: unmeasured was guessed: $(pre_row "$P" unmeasured)"
pre_ready "$P" formula local 300 || true
[ -z "$(pre_row "$P" authored)" ] || check "prefetch gpuless: authored was guessed on its author's word: $(pre_row "$P" authored)"
coming_go measured '["local"]'
coming_done measured 600
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "measured")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
if [ -n "$run" ] && J '.state == "done"' <<< "$job"; then
  echo "$P" > "$OUT/gpuless-measured.at"
  run_show "$OUT" gpuless-measured "$run"
  echo "prefetch gpuless: measured's render $run: $(jq -c '{machine, status, execution_ms, placed: (.placed | {machine, basis, warm, fetch_bytes, prepare_ms, finish_s: ((.finish_at_ms - .quoted_at_ms) / 1000)})}' "$OUT/gpuless-measured.json" 2> /dev/null)"
  J '.machine == "local" and .placed.basis == "reference" and .placed.formula_ms == null' "$OUT/gpuless-measured.json" \
    || check "prefetch gpuless: measured's render wasn't placed here on its CPU row: $(jq -c '{machine, placed}' "$OUT/gpuless-measured.json")"
else
  check "prefetch gpuless: measured's render didn't end done here: ${job:-no job}"
fi
coming_go formula '["local"]'
coming_done formula 600
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "formula")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
if [ -n "$run" ] && J '.state == "done"' <<< "$job"; then
  echo "$P" > "$OUT/gpuless-formula.at"
  run_show "$OUT" gpuless-formula "$run"
  echo "prefetch gpuless: formula's render $run: $(jq -c '{machine, status, execution_ms, placed: (.placed | {machine, basis, warm, formula_ms, size, prepare_ms, finish_s: ((.finish_at_ms - .quoted_at_ms) / 1000)})}' "$OUT/gpuless-formula.json" 2> /dev/null)"
  J '.machine == "local" and .placed.basis == "reference" and .placed.formula_ms == 8000' "$OUT/gpuless-formula.json" \
    || check "prefetch gpuless: formula's render wasn't placed here by its formula over its CPU row (8000 ms): $(jq -c '{machine, placed}' "$OUT/gpuless-formula.json")"
else
  check "prefetch gpuless: formula's render didn't end done here: ${job:-no job}"
fi
coming_go unmeasured '["local"]'
for _ in $(seq 120); do
  status "$P" > "$OUT/gpuless-status.json" 2> /dev/null || true
  J --arg l "$line" '[.queue[]? | select(.name == "unmeasured") | .no_gpu.line // "" | test($l)] | any' "$OUT/gpuless-status.json" && break
  sleep 0.5
done
job=$(jq -c '[.board.jobs[]? | select(.asset == "unmeasured")] | last // empty' "$OUT/gpuless-status.json" 2> /dev/null)
echo "prefetch gpuless: unmeasured: $(jq -c '[.queue[]? | select(.name == "unmeasured")]' "$OUT/gpuless-status.json" 2> /dev/null) ${job:-no job}"
J --arg l "$line" '[.queue[]? | select(.name == "unmeasured") | .no_gpu.line // "" | test($l)] | any' "$OUT/gpuless-status.json" \
  || check "prefetch gpuless: unmeasured's box doesn't say this computer has no GPU: ${job:-no job on the board}"
J --arg l "$line" '[.board.said[]? | .message] | any(ltrimstr("unmeasured: ") | test($l))' "$OUT/gpuless-status.json" \
  || check "prefetch gpuless: the board didn't say unmeasured waits for a GPU: $(jq -c '[.board.said[]?.message]' "$OUT/gpuless-status.json" 2> /dev/null | head -c 500)"
[ -z "$(jq -r '.run // empty' <<< "${job:-null}")" ] || check "prefetch gpuless: unmeasured's render was sent, run $(jq -r .run <<< "$job")"
coming_go authored '["local"]'
for _ in $(seq 120); do
  status "$P" > "$OUT/gpuless-status.json" 2> /dev/null || true
  J --arg l "$line" '[.queue[]? | select(.name == "authored") | .no_gpu.line // "" | test($l)] | any' "$OUT/gpuless-status.json" && break
  sleep 0.5
done
job=$(jq -c '[.board.jobs[]? | select(.asset == "authored")] | last // empty' "$OUT/gpuless-status.json" 2> /dev/null)
echo "prefetch gpuless: authored: $(jq -c '[.queue[]? | select(.name == "authored")]' "$OUT/gpuless-status.json" 2> /dev/null) ${job:-no job}"
J --arg l "$line" '[.queue[]? | select(.name == "authored") | .no_gpu.line // "" | test($l)] | any' "$OUT/gpuless-status.json" \
  || check "prefetch gpuless: authored's box doesn't say this computer has no GPU: its formula read no CPU row: ${job:-no job on the board}"
[ -z "$(jq -r '.run // empty' <<< "${job:-null}")" ] || check "prefetch gpuless: authored's render was sent on its author's word, run $(jq -r .run <<< "$job")"
sleep 20
pre_machine local "$OUT/gpuless-after.json"
[ "$(pre_held "$OUT/gpuless-after.json" e2e/pre-req)" = "$(pre_held "$OUT/gpuless-before.json" e2e/pre-req)" ] \
  || check "prefetch gpuless: e2e/pre-req landed here for unmeasured: $(pre_held "$OUT/gpuless-before.json" e2e/pre-req) then $(pre_held "$OUT/gpuless-after.json" e2e/pre-req)"
project_api "$P" /cancel -X POST -H 'Content-Type: application/json' -d '{"names":["unmeasured","authored"]}' > "$OUT/gpuless-cancel.json" 2>&1 || true
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
# Its last use three days ago, well before anything the guesses bring: least recently used by any reckoning but a guess's.
sudo find "$mnt/uses" -maxdepth 1 -type f -exec touch -d '3 days ago' {} + || check "prefetch disk: pre-ask's use wasn't made older"
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
# A request makes room: what only a guess brought goes first. Each use the store keeps, when last used, beside its checkpoint's name.
pre_machine "$name" "$OUT/pre-disk-uses.json"
sudo find "$mnt/uses" -maxdepth 1 -type f -printf '%TY-%Tm-%Td %TH:%TM %f\n' 2> /dev/null | while read -r day time digest; do
  echo "prefetch disk: use $day $time ${digest:0:12} $(jq -r --arg d "$digest" '[.status.models[]? | select([.checkpoints[]? | .manifest, .tree] | map(tostring) | any(contains($d))) | .repository] | first // "?"' "$OUT/pre-disk-uses.json" 2> /dev/null)"
done
n=$(run_submit "$OUT" pre-req model install e2e/pre-req --on "$name")
{ [ -n "$n" ] && run_wait "$OUT" pre-req && [ "$(run_get "$OUT" pre-req .status)" = completed ]; } \
  || { check "prefetch disk: model install e2e/pre-req on $name: $(run_get "$OUT" pre-req .reason.message | head -c 300)"; pre_disk_end; return 0; }
pre_machine "$name" "$OUT/pre-disk-made-room.json"
echo "prefetch disk: once e2e/pre-req came: pre-ask $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-ask), pre-a $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-a), pre-req $(pre_held "$OUT/pre-disk-made-room.json" e2e/pre-req)"
pre_log "$name" "$OUT/pre-disk-room.tensord.log"
{ grep -E "evicted|dropped|freed|making room|downloads ended" "$OUT/pre-disk-room.tensord.log" || true; } | cut -c1-260 | tail -8 | sed 's/^/prefetch disk: /'
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

# prefetch_retry (cl-1104's open items; the coordinator, 11-17: "A failed prefetch guess stays failed ... Retry it the standard way:
# transient errors back off with attempts shown; a non-recoverable error stays failed with a one-line reason; a new guess (draft
# edited, machine changed) clears it", and "A guess's install isn't in the machine's status activity, so the box's row never says
# it's installing"). A project of this computer's machine, shown by a page, gets a box of e2e-slow-install while nothing serves the
# wheel its environment takes: its guess can't connect, and fails once its machine's own tries are spent. Its row says it's tried
# again, with the attempt; the wheel's server comes (8 MiB/s), and the guess builds the environment, its row saying it's installing
# it and the machine's activity on the page naming the box, then it's ready. A box of e2e-prefetched naming a model no Hub has fails
# for good: its row says why in one line, and it isn't tried again.
prefetch_retry() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1104): a failed guess is tried again when it may not fail again, says why when it stays, and says it's installing"
pre_page || return 0
local P=$OUT/pre-retry rows=$OUT/pre-retry-rows.jsonl row='' failed='' ready='' began seen _
if curl -fsI http://127.0.0.1:47104/e2e_slow_wheel-0.0.0-py3-none-any.whl > /dev/null 2>&1; then
  check "prefetch retry: something serves e2e-slow-install's wheel already"; pre_end; return 0
fi
cozy --json machine show local > "$OUT/retry-local.json" || { check "prefetch retry: machine show local"; pre_end; return 0; }
pre_project "$P" local || { pre_end; return 0; }
: > "$rows"
jq -n '[{name: "retried", tool: "e2e-slow-install/still", kind: "image", fields: {instructions: "A still whose environment cannot be fetched yet."}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
# Its machine's own tries at the wheel are spent within a minute or two (`published::patiently`): then its run has ended.
began=$(date +%s)
while [ $(($(date +%s) - began)) -lt 300 ]; do
  row=$(pre_row "$P" retried)
  [ -z "$row" ] || echo "$row" >> "$rows"
  if J '.ready == true' <<< "${row:-null}"; then
    check "prefetch retry: the guess got ready with nothing serving its wheel: nothing failed to be tried again"; pre_end; return 0
  fi
  [ -n "$row" ] && J '.stage == null' <<< "$row" && { failed=1; break; }
  sleep 0.5
done
[ -n "$failed" ] || { check "prefetch retry: the guess's run didn't end in 5 min with nothing serving its wheel: its row: ${row:-none}"; pre_end; return 0; }
echo "prefetch retry: $(($(date +%s) - began)) s on, its run ended: $row"
python3 "$here/project/slowinstall/wheel.py" "$OUT/retry-wheel" $((8 << 20)) > "$OUT/retry-wheel.log" 2>&1 &
PRE_WHEEL=$!
began=$(date +%s)
while [ $(($(date +%s) - began)) -lt 300 ]; do
  row=$(pre_row "$P" retried)
  [ -z "$row" ] || echo "$row" >> "$rows"
  J '.ready == true' <<< "${row:-null}" && { ready=1; break; }
  sleep 0.5
done
kill "$PRE_WHEEL" 2> /dev/null || true
PRE_WHEEL=''
seen=$(jq -sc '[.[] | {stage, waiting, held_back, ready}] | unique' "$rows" 2> /dev/null)
echo "prefetch retry: its row said: ${seen:0:3000}; the wheel was asked for $(grep -c '"GET /e2e_slow_wheel' "$OUT/retry-wheel.log") times"
[ -n "$ready" ] || check "prefetch retry: the guess that failed while its wheel had no server never came again once it had one (5 min): its row: ${row:-none}"
jq -se '[.[] | select(.stage == null and (.waiting // "" | test("attempt [0-9]+/[0-9]+ in [0-9.]+ s$")))] | length > 0' "$rows" > /dev/null \
  || check "prefetch retry: once its run failed its row never said it's tried again with the attempt"
jq -se '[.[] | select(.stage == "installing it")] | length > 0' "$rows" > /dev/null \
  || check "prefetch retry: its row never said its machine was installing it while it built the environment"
# The machine as the page's stream shows it, this project's view: the guess's install among its activity, naming the box.
sse "$OUT/pre-page.sse" | jq -c 'select(.type == "machine.updated" and .machine.name == "local") | .machine.status.activity[]? | select(.kind == "install")
  | {subject, names, runs, project}' 2> /dev/null | sort -u | head -20 | sed 's/^/prefetch retry: on the page, local installs: /'
sse "$OUT/pre-page.sse" | jq -e 'select(.type == "machine.updated" and .machine.name == "local") | .machine.status.activity[]?
  | select(.kind == "install" and (.subject | test("e2e-slow-install")) and ((.names // []) | index("retried")))' > /dev/null 2>&1 \
  || check "prefetch retry: the machine's activity on the page never listed the guess's install of e2e-slow-install for its box"

# A request no try can answer: it stays failed, says why in a line, and isn't tried again.
jq -n '[{name: "missing", tool: "e2e-prefetched/still", kind: "image", fields: {instructions: "A still of a model no Hub has.", models: {model: {ref: "e2e/pre-missing@0.1.0"}}}}]' \
  | boxes_in "$P" || { pre_end; return 0; }
: > "$OUT/pre-missing-rows.jsonl"
began=$(date +%s)
while [ $(($(date +%s) - began)) -lt 180 ]; do
  row=$(pre_row "$P" missing)
  [ -z "$row" ] || echo "$row" >> "$OUT/pre-missing-rows.jsonl"
  J '.held_back != null' <<< "${row:-null}" && break
  sleep 0.5
done
echo "prefetch retry: the box of a model no Hub has: ${row:-no row}"
if J '.held_back != null' <<< "${row:-null}"; then
  J '.held_back | (test("\n") | not) and length <= 241' <<< "$row" || check "prefetch retry: why its guess stays failed isn't one line: $(jq -r .held_back <<< "$row")"
  for _ in $(seq 30); do row=$(pre_row "$P" missing); [ -z "$row" ] || echo "$row" >> "$OUT/pre-missing-rows.jsonl"; sleep 0.5; done
  jq -se '[.[] | select(.waiting // "" | test("attempt [0-9]+/[0-9]+ in"))] | length == 0' "$OUT/pre-missing-rows.jsonl" > /dev/null \
    || check "prefetch retry: a guess no try can answer was tried again: $(jq -sc '[.[] | .waiting] | unique' "$OUT/pre-missing-rows.jsonl")"
  J '.held_back != null and .ready != true' <<< "${row:-null}" || check "prefetch retry: the guess no try can answer stopped saying why it failed: ${row:-no row}"
else
  check "prefetch retry: its row never said why its guess failed in 3 min: ${row:-no row}"
fi
pre_end
}

# prefetch_order (cl-1278; the Predictive Downloads coordinator, 11-18, on Paul's "you know how google chrome, when you're typing into a
# browser, will pre-fetch the url you're likely to complete typing?"): "among a machine's predicted boxes, the one the user touched
# most recently downloads first." The release walkthrough (pod-session 44338350009, one RTX 4090): two cold boxes, about 60 GB at
# about 105 MB/s; an edit makes a new item, which went by its run's acceptance, so the Qwen box just edited went behind Klein's whole
# set, and its Render waited about 100 s for an 11 GB enhancer that hadn't started. Here stand-o, a new stand-in, holds nothing, and
# the store is at 10 MB/s. A project of it holds two boxes when the page shows it: `first` (e2e/pre-a: its 320 MiB VAE and 384 MiB
# transformer) and `second` (e2e/pre-b's transformer, 384 MiB, on that VAE). `first` downloads; with half of what it needs there
# (352 MiB), `second`'s words are edited. The edited box's needs are whole before `first`'s. Nothing of `first` is cancelled: its item stays
# listed, and each of its objects in flight at the edit is begun once, never refused, and lands. The timeline is the machine's own
# (stand-o's TensorD's log): when each item's needs were here and each object began and landed, from the edit.
prefetch_order() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1278): of two boxes guessed onto one machine, the one edited last downloads first, and nothing of the other is cancelled"
local P=$OUT/pre-order name=stand-o tool=e2e-prefetched/still began edited came at_edit facts
[ -z "${PRE_PAGE:-}${PRE_SLOWED:-}" ] || pre_end
PRE_PAGE='' PRE_SLOWED=''
cozy --json machine show local > "$OUT/set-local.json" || { check "prefetch order: machine show local"; return 0; }
stand_in "$name" || return 0
sudo tc qdisc del dev lo root 2> /dev/null || true
slow_store prefetch 80mbit || { order_end; return 0; }
PRE_SLOWED=1
cozy --json new --unencrypted "$P" > "$OUT/pre-order-new.json" 2>&1 || { check "prefetch order: new: $(tail -c 300 "$OUT/pre-order-new.json")"; order_end; return 0; }
in_project "$P" --json machine use "$name" > "$OUT/pre-order-use.json" 2>&1 || { check "prefetch order: machine use $name: $(tail -c 300 "$OUT/pre-order-use.json")"; order_end; return 0; }
# Both boxes there before the page shows the project: one guess round makes both, in the boxes' order.
jq -n --arg t "$tool" '[{name: "first", tool: $t, kind: "image", fields: {instructions: "A still of the first box."}},
  {name: "second", tool: $t, kind: "image", fields: {instructions: "A still of the second box.", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' \
  | boxes_in "$P" || { order_end; return 0; }
pre_page || { order_end; return 0; }
began=$(date -u +%s%3N)
pre_show "$P" || { order_end; return 0; }
came=$(pre_wait "$P" first 352)
at_edit="first $(pre_row "$P" first | jq -c '{done, total, ready, stage}' 2> /dev/null), second $(pre_row "$P" second | jq -c '{done, total, ready, stage}' 2> /dev/null)" || true
[ "$came" -ge $((352 << 20)) ] || { check "prefetch order: $name never had 352 MiB of the first box's needs: $at_edit"; order_end; return 0; }
edited=$(date -u +%s%3N)
project_api "$P" /assets/second -X PATCH -H 'Content-Type: application/json' \
  -d '{"fields": {"instructions": "A still of the second box, at dusk.", "models": {"model": {"ref": "e2e/pre-b@0.1.0"}}}}' > "$OUT/pre-order-patch.json" 2>&1 \
  || check "prefetch order: the second box wasn't edited: $(tail -c 300 "$OUT/pre-order-patch.json")"
echo "prefetch order: the second box was edited $((edited - began)) ms after the page showed the project; the rows then: $at_edit"
pre_ready "$P" second "$name" 300 || true
pre_ready "$P" first "$name" 300 || true
cozy machine logs local calcifer --lines 0 --since 30m > "$OUT/pre-order-calcifer.log" 2>&1 || true
pre_log "$name" "$OUT/pre-order.tensord.log"
# Which item is whose, from the lists Calcifer sent stand-o: the key on the list before the edit and after it is the first box's,
# the key new after it the edited box's.
facts=$(python3 -I - "$OUT/pre-order-calcifer.log" "$name" "$began" "$edited" <<'KEYS'
import json, re, sys
from datetime import datetime
log, name, began, edited = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
lists = []
for line in open(log, errors="replace"):
    found = re.search(r"lists to " + re.escape(name) + r": .*prepare \[(.*?)\]( \(unchanged\))?$", line.rstrip())
    if not found or found[2]:
        continue
    try:
        when = int(datetime.fromisoformat(line.split(" ", 1)[0].replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        continue
    if when >= began:
        lists.append((when, re.findall(r'"([^"]+)"', found[1])))
before = next((keys for when, keys in reversed(lists) if when <= edited), [])
after = next(((when, keys) for when, keys in lists if when > edited and set(keys) - set(before)), (0, []))
kept, new = [key for key in before if key in after[1]], [key for key in after[1] if key not in before]
print(json.dumps({"edited": edited, "before": before, "after": after[1], "listed_ms": after[0] - edited if after[0] else None,
                  "first": kept[0] if len(kept) == 1 else "", "second": new[0] if len(new) == 1 else "", "lists": len(lists)}))
KEYS
) || facts=''
echo "prefetch order: Calcifer's lists to $name: $(jq -c '{before, after, listed_ms, lists}' <<< "${facts:-null}" 2> /dev/null)"
if J '(.first // "") != "" and (.second // "") != ""' <<< "${facts:-null}"; then
  pre_judge "$OUT/pre-order.tensord.log" "$began" "$facts" <<'JUDGE' > "$OUT/pre-order.failed" 2> "$OUT/pre-order.timeline"
edit, here, dropped = facts["edited"], {}, []
for line in open(log, errors="replace"):
    when = at(line)
    if when < since:
        continue
    found = re.search(r"cozy:prepare:(\S+): what it needs is here", line)
    if found:
        here.setdefault(found[1], when)
    found = re.search(r"list item cozy:prepare:(\S+) dropped: its run", line)
    if found:
        dropped.append(found[1])
first, second = here.get(facts["first"]), here.get(facts["second"])
said = lambda when: "never" if when is None else f"{(when - edit) / 1000:+.1f} s"
place = lambda seen: (max(seen.get("order", 2) - 2, 0) >> 32) & ((1 << 30) - 1)
# Each object in the order it began: when it began and landed from the edit, its size, and its place in the line and rank there.
for sha, seen in sorted(weights.items(), key=lambda item: min(item[1]["fetching"] or [1 << 62])):
    print(f"  {sha[:12]} {seen['bytes'] // MIB:4d} MiB began {said(min(seen['fetching'], default=None))}, landed {said(min(seen['landed'], default=None))}"
          f", place {place(seen)} rank {max(seen.get('order', 2) - 2, 0) & 0xffffffff}, begun {len(seen['fetching'])}x, refused {len(seen['refused'])}x", file=sys.stderr)
flying = [seen for seen in weights.values() if seen["fetching"] and min(seen["fetching"]) <= edit and not (seen["landed"] and min(seen["landed"]) <= edit)]
went = sum(seen["bytes"] for seen in weights.values() if seen["landed"] and min(seen["landed"]) <= edit)
print(json.dumps({"edited_box_whole": said(second), "first_box_whole": said(first), "landed_at_the_edit_mib": went // MIB, "in_flight_at_the_edit": len(flying),
                  "of_them_landed_before_the_edited_box_was_whole": sum(1 for seen in flying if seen["landed"] and second and min(seen["landed"]) <= second),
                  "begun_again": sum(1 for seen in weights.values() if len(seen["fetching"]) > 1), "refused": sum(1 for seen in weights.values() if seen["refused"])}), file=sys.stderr)
if second is None or first is None:
    print(f"what the edited box needs was here {said(second)} and what the first box needs {said(first)}: one never came whole")
elif second > first:
    print(f"what the edited box needs was whole {(second - first) / 1000:.1f} s after what the first box needs ({said(second)} against {said(first)} from the edit): the box touched last went behind the box guessed before it")
if not flying:
    print("nothing was in flight when the second box was edited: the first box wasn't downloading, and the scene doesn't hold")
again = [seen for seen in flying if len(seen["fetching"]) > 1 or seen["refused"]]
if again:
    print(f"{len(again)} of the first box's {len(flying)} objects in flight at the edit were begun again or refused: something of it was cancelled")
lost = [seen for seen in flying if not seen["landed"]]
if lost:
    print(f"{len(lost)} of the first box's {len(flying)} objects in flight at the edit never landed")
if facts["first"] in dropped:
    print("the first box's item was dropped and its run ended, though only the second box was edited")
runs = [sha[:12] for sha, seen in weights.items() if seen["class"] != 3]
if runs:
    print(f"{len(runs)} objects came at a run's order, not a caller's guess's (class 3): {runs[:3]}")
JUDGE
  sed 's/^/prefetch order: /' "$OUT/pre-order.timeline"
  grep -q '"edited_box_whole"' "$OUT/pre-order.timeline" && ! grep -q '^Traceback' "$OUT/pre-order.timeline" \
    || check "prefetch order: its judge didn't finish: $(tail -2 "$OUT/pre-order.timeline" | cut -c1-300)"
  while read -r why; do check "prefetch order: $why"; done < "$OUT/pre-order.failed"
else
  check "prefetch order: Calcifer's lists to $name don't say which item is whose: $(grep -E "lists to $name" "$OUT/pre-order-calcifer.log" | tail -4 | cut -c1-300)"
fi
order_end
}
# order_end: the page gone, the link as it was, stand-o ended.
order_end() {
  pre_end
  timeout 60 "$COZY" --json machine end stand-o --force -y > "$OUT/stand-o-end.json" 2>&1 || true
}

# prefetch_sealed (cl-1123; Paul, 11-18: "even for encrypted projects, sending ahead of time is fine. What really matters is that we can
# trust the remote machine as safe"): an encrypted project's draft never goes ahead to a computer added by address, which isn't trusted
# with its content until Paul rules it is, while a plain project's still does. stand-s, a stand-in, is such a computer. An encrypted
# project of it shows sealpic (its picture under e2e-prefetched/still, on e2e/pre-b's transformer, its draft holding a marker) and
# sealown (a box of the project's own code); a plain project of it shows openpic (the same tool on e2e/pre-a). Shown together, openpic
# is listed on stand-s and its picture carried there. Nothing of the encrypted project is listed there: no row, stand-s fetches none
# of e2e/pre-b and names neither its picture nor its draft, and the project's own log says why. Calcifer's log names nothing of the
# encrypted project. sealpic's Render still goes to stand-s and runs.
prefetch_sealed() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1123): an encrypted project's draft doesn't go ahead to a computer added by address; a plain one's does"
local name=stand-s sealed=$OUT/sealedcarry open=$OUT/opencarry secrets view sid oid shex ohex mark said walk held row='' job box P _
cozy --json machine show local > "$OUT/set-local.json" || { check "sealed: machine show local"; return 0; }
stand_in "$name" || return 0
secrets=$(mktemp -d)
printf '%s\n' 'synthetic sealed-guess passphrase' > "$secrets/pass" && chmod 600 "$secrets/pass"
cozy --json new "$sealed" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$OUT/sealed-new.json" 2> "$OUT/sealed-new.err" \
  || { check "sealed: new encrypted project: $(tail -c 300 "$OUT/sealed-new.err")"; sealed_end; return 0; }
view=$(jq -r '.view // empty' "$OUT/sealed-new.json")
[ -n "$view" ] && [ -d "$view" ] || { check "sealed: no unlocked view: $(head -c 300 "$OUT/sealed-new.json")"; sealed_end; return 0; }
cozy --json new --unencrypted "$open" > "$OUT/open-new.json" 2>&1 || { check "sealed: new plain project: $(tail -c 300 "$OUT/open-new.json")"; sealed_end; return 0; }
for P in "$sealed" "$open"; do
  in_project "$P" --json machine use "$name" > "$OUT/sealed-use.json" 2>&1 || { check "sealed: machine use $name: $(tail -c 300 "$OUT/sealed-use.json")"; sealed_end; return 0; }
done
# Two pictures of noise, 3 MiB each: past a carry's 1 MiB batch, so each lands in parts, its digest named in the machine's log.
python3 -I - "$OUT/sealed-pic.png" "$OUT/open-pic.png" <<'PY'
import os, struct, sys, zlib
side = 1024
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
for path in sys.argv[1:]:
    rows = b"".join(b"\0" + os.urandom(side * 3) for _ in range(side))
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))
PY
shex=$(sha256sum "$OUT/sealed-pic.png" | cut -c1-64) ohex=$(sha256sum "$OUT/open-pic.png" | cut -c1-64) mark=sealdraft$RANDOM$RANDOM
python3 "$E2E/page.py" import "$sealed" "$OUT/sealed-pic.png" --name sealpic > "$OUT/sealpic.json" 2>&1 \
  && python3 "$E2E/page.py" set "$sealed" sealpic --tool e2e-prefetched/still \
    --fields "$(jq -nc --arg m "$mark" '{instructions: "<self.1> at dusk, \($m)", models: {model: {ref: "e2e/pre-b@0.1.0"}}}')" > "$OUT/sealpic-set.json" 2>&1 \
  || { check "sealed: sealpic: $(tail -c 300 "$OUT/sealpic.json") $(tail -c 300 "$OUT/sealpic-set.json" 2> /dev/null)"; sealed_end; return 0; }
cp -r "$here/project/boxed" "$view/boxed" || { check "sealed: its own code not copied into its view"; sealed_end; return 0; }
python3 "$E2E/page.py" add "$sealed" sealown --tool ./boxed/plain --fields '{"instructions": "a still, a shade lighter"}' > "$OUT/sealown.json" 2>&1 \
  || { check "sealed: sealown: $(tail -c 300 "$OUT/sealown.json")"; sealed_end; return 0; }
python3 "$E2E/page.py" import "$open" "$OUT/open-pic.png" --name openpic > "$OUT/openpic.json" 2>&1 \
  && python3 "$E2E/page.py" set "$open" openpic --tool e2e-prefetched/still --fields '{"instructions": "<self.1> at dusk"}' > "$OUT/openpic-set.json" 2>&1 \
  || { check "sealed: openpic: $(tail -c 300 "$OUT/openpic.json") $(tail -c 300 "$OUT/openpic-set.json" 2> /dev/null)"; sealed_end; return 0; }
echo "sealed: sealpic's picture sha256:$shex, openpic's sha256:$ohex, $(stat -c %s "$OUT/sealed-pic.png") bytes each"
pre_page || { sealed_end; return 0; }
sid=$(status "$sealed" | jq -r '.id // empty') oid=$(status "$open" | jq -r '.id // empty')
[ -n "$sid" ] && [ -n "$oid" ] || { check "sealed: the projects' ids: '$sid' '$oid'"; sealed_end; return 0; }
sealed_show "[\"$sid\", \"$oid\"]" || { sealed_end; return 0; }

# The plain project: listed on stand-s, its picture carried there.
for _ in $(seq 360); do row=$(pre_row "$open" openpic); J --arg m "$name" '.machine == $m' <<< "${row:-null}" && break; sleep 0.5; done
echo "sealed: openpic's row: ${row:-none}"
J --arg m "$name" '.machine == $m' <<< "${row:-null}" || { check "sealed: the plain project's openpic was never listed on $name in 3 min: ${row:-none}"; sealed_end; return 0; }
for _ in $(seq 90); do
  cozy machine logs local calcifer --lines 20000 > "$OUT/sealed-calcifer.log" 2>&1 || true
  grep -qE "openpic's guess carried [0-9]+ files .* ahead to $name" "$OUT/sealed-calcifer.log" && break
  sleep 2
done
grep -E "openpic's guess carried" "$OUT/sealed-calcifer.log" | tail -2 | cut -c1-400 | sed 's/^/sealed: Calcifer: /' || true
grep -qE "openpic's guess carried [0-9]+ files .* ahead to $name" "$OUT/sealed-calcifer.log" || check "sealed: openpic's picture was never carried to $name (3 min)"

# The encrypted project: nothing of it listed there, and its own log says why.
said="nothing of this encrypted project goes ahead: $name is another computer's, which this account doesn't own"
walk=$view/.vault-calcifer/diagnostics/walk.log
for _ in $(seq 60); do grep -qF "sealpic isn't guessed: $said" "$walk" 2> /dev/null && grep -qF "sealown isn't guessed: $said" "$walk" 2> /dev/null && break; sleep 2; done
grep -E "sealpic|sealown" "$walk" 2> /dev/null | tail -6 | cut -c1-400 | sed 's/^/sealed: its vault log: /' || true
for box in sealpic sealown; do
  grep -qF "$box isn't guessed: $said" "$walk" 2> /dev/null || check "sealed: the encrypted project's own log doesn't say why $box isn't guessed onto $name"
  row=$(pre_row "$sealed" "$box")
  [ -z "$row" ] || check "sealed: the encrypted project's $box is listed on $name, a computer added by address: $row"
done
sleep 10
pre_machine "$name" "$OUT/sealed-$name.json"
held=$(pre_held "$OUT/sealed-$name.json" e2e/pre-b)
echo "sealed: of e2e/pre-b, which only sealpic's draft reads, $name holds $held before any render"
J 'all(.unique_bytes == 0)' <<< "$held" || check "sealed: $name fetched e2e/pre-b for the encrypted project's draft before any render: $held"
pre_log "$name" "$OUT/sealed-$name.log"
! grep -q "$shex" "$OUT/sealed-$name.log" || check "sealed: $name's log names sealpic's picture before any render"
! grep -qF "$mark" "$OUT/sealed-$name.log" || check "sealed: $name's log holds the encrypted draft's words before any render"
! grep -qi "e2e.boxed" "$OUT/sealed-$name.log" || check "sealed: $name's log names the encrypted project's own code (e2e-boxed)"
cozy machine logs local calcifer --lines 20000 > "$OUT/sealed-calcifer.log" 2>&1 || true
! grep -qE "sealpic|sealown|sealedcarry|$shex|$mark" "$OUT/sealed-calcifer.log" \
  || check "sealed: Calcifer's log names the encrypted project: $(grep -E "sealpic|sealown|sealedcarry|$shex|$mark" "$OUT/sealed-calcifer.log" | head -2 | cut -c1-300)"

# Its Render isn't held: sealpic renders on stand-s.
timeout 900 python3 "$E2E/page.py" render "$sealed" --on "$name" sealpic > "$OUT/sealed-render.txt" 2>&1 \
  || check "sealed: sealpic's render on $name: $(tail -c 400 "$OUT/sealed-render.txt")"
job=$(status "$sealed" | jq -c '[.board.jobs[]? | select(.asset == "sealpic")] | last // empty')
echo "sealed: sealpic's render: ${job:-no job}"
J --arg m "$name" '.machine == $m and .state == "done"' <<< "${job:-null}" || check "sealed: sealpic's render didn't end done on $name: ${job:-no job}"
sealed_end
}
# sealed_show <ids JSON>: the page shows those projects. sealed_end: the page gone, stand-s ended, the encrypted project locked.
sealed_show() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  curl -fsS -X PUT "${PRE_HEADERS[@]}" -H "Origin: http://$(jq -r .address "$lock")" -H 'Content-Type: application/json' \
    -d "$(jq -nc --argjson ids "$1" '{projects: $ids}')" "http://$(jq -r .address "$lock")/v1/events/$PRE_STREAM" > "$OUT/sealed-show.json" 2>&1 \
    || { check "sealed: the page couldn't show $1: $(tail -c 300 "$OUT/sealed-show.json")"; return 1; }
}
sealed_end() {
  [ -z "${PRE_PAGE:-}" ] || kill "$PRE_PAGE" 2> /dev/null || true
  PRE_PAGE=''
  timeout 60 "$COZY" --json machine end stand-s --force -y > "$OUT/stand-s-end.json" 2>&1 || true
  in_project "$OUT/sealedcarry" --json vault lock > /dev/null 2>&1 || true
}

# prefetch_sealed_demand (cl-1123; the release coordinator, 11-18: "for an encrypted project the request should be on the machine that
# runs the run and nowhere else; that the user listed that computer among the machines a run could go to is consent to it RUNNING the
# work, not to it holding a copy of a request another machine is running"). A job waiting on one machine is prepared on an idle one
# that could run it (cl-967's demand), but an encrypted project's never on a computer added by address. An encrypted project and a
# plain one each run their tool on local once (`warm`), then render `src`, a 4-minute still, on local, and `after`, e2e-prefetched/still
# on `<src>`, on local or stand-d, a stand-in added by address that runs nothing: planned on local, where its model is, `after` waits
# there. The plain project's (e2e/pre-a) is prepared on stand-d. The encrypted one's (e2e/pre-b, its words a marker) isn't: stand-d's
# prepare list never holds more than the plain one's item, stand-d fetches none of pre-b and its log holds no marker, the project's
# own log says why, and Calcifer's log names nothing of it. Both `after`s then run on local.
prefetch_sealed_demand() {
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || return 0
step "prefetch (cl-1123): an encrypted project's job waiting on one machine isn't prepared on a computer added by address; a plain one's is"
local name=stand-d S=$OUT/sealeddemand O=$OUT/opendemand wait=$here/project/waits.py/wait secrets view mark job box P most said walk
cozy --json machine show local > "$OUT/set-local.json" || { check "sealed demand: machine show local"; return 0; }
stand_in "$name" || return 0
secrets=$(mktemp -d)
printf '%s\n' 'synthetic sealed-demand passphrase' > "$secrets/pass" && chmod 600 "$secrets/pass"
cozy --json new "$S" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$OUT/sd-new.json" 2> "$OUT/sd-new.err" \
  || { check "sealed demand: new encrypted project: $(tail -c 300 "$OUT/sd-new.err")"; sd_end; return 0; }
view=$(jq -r '.view // empty' "$OUT/sd-new.json")
[ -n "$view" ] && [ -d "$view" ] || { check "sealed demand: no unlocked view: $(head -c 300 "$OUT/sd-new.json")"; sd_end; return 0; }
cozy --json new --unencrypted "$O" > "$OUT/od-new.json" 2>&1 || { check "sealed demand: new plain project: $(tail -c 300 "$OUT/od-new.json")"; sd_end; return 0; }
mark=sealdemand$RANDOM$RANDOM
jq -n --arg w "$wait" --arg m "$mark" '[
  {name: "sdwarm", tool: "e2e-prefetched/still", kind: "image", fields: {instructions: "a warm still", models: {model: {ref: "e2e/pre-b@0.1.0"}}}},
  {name: "sdsrc", tool: $w, kind: "image", fields: {instructions: "a still that takes 4 min", megapixels: 0.25, seconds: 240}},
  {name: "sdafter", tool: "e2e-prefetched/still", kind: "image", fields: {instructions: "<sdsrc> at dusk, \($m)", models: {model: {ref: "e2e/pre-b@0.1.0"}}}}]' \
  | boxes_in "$S" || { sd_end; return 0; }
jq -n --arg w "$wait" '[
  {name: "odwarm", tool: "e2e-prefetched/still", kind: "image", fields: {instructions: "a warm still"}},
  {name: "odsrc", tool: $w, kind: "image", fields: {instructions: "a still that takes 4 min", megapixels: 0.25, seconds: 240}},
  {name: "odafter", tool: "e2e-prefetched/still", kind: "image", fields: {instructions: "<odsrc> at dusk"}}]' \
  | boxes_in "$O" || { sd_end; return 0; }
# Each project's tool run on local first: local holds its model and has timed it there.
sd_go "$S" sdwarm '["local"]'
sd_go "$O" odwarm '["local"]'
for box in sdwarm odwarm; do
  P=$S; [ "$box" = sdwarm ] || P=$O
  job=$(sd_done "$P" "$box" 300)
  J '.state == "done" and .machine == "local"' <<< "${job:-null}" || { check "sealed demand: $box didn't run on local: ${job:-no job}"; sd_end; return 0; }
done
sd_go "$S" sdsrc '["local"]'
sd_go "$O" odsrc '["local"]'
sd_go "$S" sdafter "[\"local\",\"$name\"]"
sd_go "$O" odafter "[\"local\",\"$name\"]"
# The plain project's waiting job prepared on stand-d: its prepare list holds an item.
for _ in $(seq 115); do
  pre_log "$name" "$OUT/sd-$name.log"
  grep -qE 'cozy:prepare: list: [1-9][0-9]* items admitted' "$OUT/sd-$name.log" && break
  sleep 2
done
sleep 20
pre_log "$name" "$OUT/sd-$name.log"
pre_machine "$name" "$OUT/sd-$name.json"
grep -hE "(sdafter|odafter) waits; fetched ahead on" "$view/.vault-calcifer/diagnostics/walk.log" "$O/.vault-calcifer/diagnostics/walk.log" 2> /dev/null | tail -4 | cut -c1-400 | sed 's/^/sealed demand: /' || true
grep -E 'cozy:(prepare|upcoming): list: [0-9]+ items admitted' "$OUT/sd-$name.log" | tail -6 | cut -c1-200 | sed "s/^/sealed demand: $name: /" || true
! grep -qE 'cozy:upcoming: list: [1-9][0-9]* items admitted' "$OUT/sd-$name.log" \
  || check "sealed demand: a job was planned on $name, which was to run nothing: the scene doesn't hold"
most=$(grep -oE 'cozy:prepare: list: [0-9]+ items admitted' "$OUT/sd-$name.log" | grep -oE '[0-9]+' | sort -n | tail -1)
echo "sealed demand: $name's prepare list held at most ${most:-no} items; of e2e/pre-a it holds $(pre_held "$OUT/sd-$name.json" e2e/pre-a), of e2e/pre-b $(pre_held "$OUT/sd-$name.json" e2e/pre-b)"
[ "${most:-0}" -ge 1 ] || check "sealed demand: the plain project's odafter, waiting on local, was never prepared on $name"
[ "${most:-0}" -le 1 ] || check "sealed demand: $name's prepare list held $most items: the encrypted project's sdafter was listed there beside the plain one's"
J 'all(.unique_bytes == 0)' <<< "$(pre_held "$OUT/sd-$name.json" e2e/pre-b)" \
  || check "sealed demand: $name fetched e2e/pre-b, which only the encrypted project's sdafter reads: $(pre_held "$OUT/sd-$name.json" e2e/pre-b)"
! grep -qF "$mark" "$OUT/sd-$name.log" || check "sealed demand: $name's log holds the encrypted request's words"
said="sdafter isn't prepared on $name while it waits elsewhere: nothing of this encrypted project goes ahead: $name is another computer's, which this account doesn't own"
walk=$view/.vault-calcifer/diagnostics/walk.log
grep -F "isn't prepared on" "$walk" 2> /dev/null | tail -2 | cut -c1-400 | sed 's/^/sealed demand: its vault log: /' || true
grep -qF "$said" "$walk" 2> /dev/null || check "sealed demand: the encrypted project's own log doesn't say why sdafter isn't prepared on $name"
[ "$(grep -cF "$said" "$walk" 2> /dev/null)" -le 1 ] || check "sealed demand: its own log said why more than once: $(grep -cF "$said" "$walk")"
cozy machine logs local calcifer --lines 20000 > "$OUT/sd-calcifer.log" 2>&1 || true
! grep -qE "sdwarm|sdsrc|sdafter|sealeddemand|$mark" "$OUT/sd-calcifer.log" \
  || check "sealed demand: Calcifer's log names the encrypted project: $(grep -E "sdwarm|sdsrc|sdafter|sealeddemand|$mark" "$OUT/sd-calcifer.log" | head -2 | cut -c1-300)"
# Both waiting jobs run where they were planned.
for box in sdafter odafter; do
  P=$S; [ "$box" = sdafter ] || P=$O
  job=$(sd_done "$P" "$box" 600)
  echo "sealed demand: $box: ${job:-no job}"
  J '.state == "done" and .machine == "local"' <<< "${job:-null}" || check "sealed demand: $box didn't end done on local: ${job:-no job}"
done
sd_end
}
# sd_go <project> <box> <machines JSON>: renders it as the page does. sd_done <project> <box> <seconds>: its job once done or failed,
# within that. sd_end: stand-d ended, the encrypted project locked.
sd_go() {
  project_api "$1" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: sd-$2-$RANDOM$RANDOM" \
    -d "{\"names\":[\"$2\"],\"on\":$3}" > "$OUT/sd-go-$2.json" 2>&1 || check "sealed demand: render $2: $(tail -c 300 "$OUT/sd-go-$2.json")"
}
sd_done() {
  local job='' _
  for _ in $(seq $(($3 * 2))); do
    job=$(status "$1" | jq -c --arg a "$2" '[.board.jobs[]? | select(.asset == $a)] | last // empty' 2> /dev/null || true)
    J '.state == "done" or .state == "failed"' <<< "${job:-null}" && break
    sleep 0.5
  done
  echo "${job:-}"
}
sd_end() {
  timeout 60 "$COZY" --json machine end stand-d --force -y > "$OUT/stand-d-end.json" 2>&1 || true
  in_project "$OUT/sealeddemand" --json vault lock > /dev/null 2>&1 || true
}
