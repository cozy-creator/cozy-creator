# e2e/cli/project.sh: a project on this computer through Calcifer's HTTP surface (cl-758): the helpers the cli suite's cells use, its
# walk (`project_stream`), GET /v1/events and the live views on it. No model runs: pictures come from a weightless local function
# (project/draw.py) and shots from project/shots.py, so every path but a model's is real.
# `version_skew` is called by run.sh's start, between the installer's cozy and the candidates: the candidate CLI against the release's
# running Calcifer.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# A fixture chooses its project by running the actual CLI in that directory.
in_project() { local directory=$1; shift; (cd "$directory" && cozy "$@"); }
# Fixture edits use the project HTTP API that the browser uses.
project_api() {
  local directory=$1 suffix=$2; shift 2
  cozy --json machine ls > /dev/null || return
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder encoded arg wrote=
  encoded=$(python3 -c 'from urllib.parse import quote; import os,sys; print(quote(os.path.abspath(sys.argv[1]), safe=""))' "$directory")
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token "$holder")" -H 'cozy-project-storage: 3' "$@" "http://$(jq -r .address "$holder")/v1/projects/$encoded$suffix" || return
  for arg in "$@"; do case $arg in POST|PATCH|PUT|DELETE|-d|--data*) wrote=1 ;; esac; done
  [ -z "$wrote" ] || project_integrity "$directory" "$suffix"
}
# The rows a read takes against the document they were written with (cl-885): after any write, nowhere different. A Calcifer from
# before the check (0.24.0's, in the cells that make a project with it) serves no such route, and is no Calcifer under test.
project_integrity() {
  local directory=$1 after=${2:-a write} holder=$HOME/.cozy/calcifer/calcifer.lock.holder encoded found code
  encoded=$(python3 -c 'from urllib.parse import quote; import os,sys; print(quote(os.path.abspath(sys.argv[1]), safe=""))' "$directory")
  found=$(curl -sS -w '\n%{http_code}' -H "Authorization: Bearer $(jq -r .token "$holder")" -H 'cozy-project-storage: 3' "http://$(jq -r .address "$holder")/v1/projects/$encoded/integrity") \
    || { check "rows_equal_document after $after in $directory: the integrity read failed: $found"; return 0; }
  code=${found##*$'\n'} found=${found%$'\n'*}
  if [ "$code" = 404 ] && [ "$(jq -r '.error.code? // empty' <<< "$found" 2> /dev/null)" = unknown_route ]; then
    return 0
  elif [ "$code" != 200 ]; then
    check "rows_equal_document after $after in $directory: the integrity read answered $code: $found"
  elif [ "$(jq '.differences | length' <<< "$found")" != 0 ]; then
    check "rows_equal_document after $after in $directory: $(jq -c .differences <<< "$found")"
  fi
  return 0
}
# project_upload <folder> <file>: the file into that project's own content store, as the page sends it; its sha256:.
project_upload() {
  local directory=$1 file=$2 source holder=$HOME/.cozy/calcifer/calcifer.lock.holder encoded address token said code
  cozy --json machine ls > /dev/null || return
  source="sha256:$(sha "$file")"
  encoded=$(python3 -c 'from urllib.parse import quote; import os,sys; print(quote(os.path.abspath(sys.argv[1]), safe=""))' "$directory")
  address=$(jq -r .address "$holder") token=$(jq -r .token "$holder") said=$(mktemp)
  code=$(curl -sS -o "$said" -w '%{http_code}' -X PUT -H "Authorization: Bearer $token" -H 'cozy-project-storage: 3' \
    --data-binary "@$file" "http://$address/v1/project-files/$source?project=$encoded") || { rm -f "$said"; return 1; }
  # A Calcifer from before project storage (0.24.0, in a migration cell's place) has no such route: it holds files for any project.
  if [ "$code" = 404 ] && jq -e '.error.code == "unknown_route"' "$said" > /dev/null 2>&1; then
    curl --fail-with-body -sS -X PUT -H "Authorization: Bearer $token" --data-binary "@$file" "http://$address/v1/files/$source" > /dev/null || { rm -f "$said"; return 1; }
  elif [ "${code:0:1}" != 2 ]; then
    cat "$said" >&2; rm -f "$said"; return 1
  fi
  rm -f "$said"
  echo "$source"
}
project_import() {
  local directory=$1 file=$2 name=${3:-} source
  source=$(project_upload "$directory" "$file") || return
  project_api "$directory" /imports -X POST -H 'Content-Type: application/json' \
    -d "$(jq -n --arg source "$source" --arg name "$name" '{source: $source} + (if $name == "" then {} else {name: $name, exact: true} end)')"
}

project_history_step() { project_api "$1" "/history/$2" -X POST -H 'Content-Type: application/json' -d "$(jq -n --argjson entry "$3" '{entry:$entry}')"; }

# Controller health is read through its API; `cozy status` describes project assets.
fixture_calcifer() {
  local machines holder=$HOME/.cozy/calcifer/calcifer.lock.holder status
  machines=$(cozy --json machine ls) || return
  status=$(curl -fsS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$(jq -r .address "$holder")/v1/status") || return
  jq -n --argjson status "$status" --argjson machines "$machines" '{calcifer: ($status + {running: true}), machines: $machines.machines}'
}
# The projects Calcifer knows.
fixture_projects() {
  cozy --json machine ls > /dev/null || return
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder
  curl -fsS -H "Authorization: Bearer $(jq -r .token "$holder")" -H 'cozy-project-storage: 3' "http://$(jq -r .address "$holder")/v1/projects"
}
# The starter film is fixture data, created through Calcifer's existing project API.
starter_project() {
  local directory=$1 holder=$HOME/.cozy/calcifer/calcifer.lock.holder
  mkdir -p "$directory" || return
  directory=$(cd "$directory" && pwd)
  [ "${RUNNER_OS:-}" != Windows ] || directory=$(cygpath -w "$directory")
  cozy --json machine ls > /dev/null || return
  curl -fsS -H "Authorization: Bearer $(jq -r .token "$holder")" -H 'cozy-project-storage: 3' -H 'Content-Type: application/json' \
    -d "$(jq -n --arg path "$directory" '{path: $path, starter: true, unencrypted: true}')" "http://$(jq -r .address "$holder")/v1/projects"
}

# The candidate CLI from its own path against the release's Calcifer: it works where that Calcifer serves the route (every
# release since 0.16.3 does `project ls`), and otherwise a typed calcifer_outdated with its remedy, never a bare 404.
version_skew() {
  [ -f "$CAND_COZY" ] || return 0
  step "version skew: the candidate CLI against the release's Calcifer"
  local code=0
  (cd "$OUT/project" && "$CAND_COZY" status) > "$OUT/skew.txt" 2>&1 || code=$?
  cat "$OUT/skew.txt"
  { { [ "$code" = 0 ] || grep -q calcifer_outdated "$OUT/skew.txt"; } && ! grep -qiE 'untyped_answer|answered 404' "$OUT/skew.txt"; } \
    || check "skew: the candidate CLI against $(cozy --version)'s Calcifer neither worked nor said calcifer_outdated (exit $code)"
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder address capability fresh=$OUT/skew-new-$RANDOM
  address=$(jq -r .address "$holder")
  capability=$(curl -fsS -H "Authorization: Bearer $(jq -r .token "$holder")" "http://$address/v1/status" | jq -r '.encrypted_project_creation // false')
  if [ "$capability" != true ]; then
    # The missing file also proves the capability refusal precedes reading a credential.
    code=0
    "$CAND_COZY" --json new "$fresh" --passphrase-file "$fresh.missing" > "$OUT/skew-encryption.txt" 2>&1 || code=$?
    { [ "$code" != 0 ] && grep -q calcifer_outdated "$OUT/skew-encryption.txt" && [ ! -e "$fresh" ]; } \
      || check "skew: unsupported encrypted creation was not refused before credential read/project creation"
  fi
}

# calcifer_record <field>: the running Calcifer's lock record's field, after a command made sure one runs (a candidate's update may
# have just restarted it); empty when there is none. (`held` is a project asset's, below.)
calcifer_record() {
  cozy --json machine ls > /dev/null 2>&1 || true
  jq -r ".$1 // empty" "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null || true
}

# open_page: a page as the browser makes one, against the running Calcifer: a sign-in code traded for a session, its events
# stream held open in the background ($PAGE_PID, writing $OUT/page.sse). close_page ends it.
open_page() {
  local address code session token
  address=$(calcifer_record address) token=$(calcifer_record token)
  [ -n "$address" ] || { check "page: no Calcifer runs"; return 1; }
  code=$(curl -fsS -X POST -H "Authorization: Bearer $token" -H 'cozy-project-storage: 3' "http://$address/v1/session-codes" | jq -r .code) &&
    session=$(curl -fsS -X POST -H 'Content-Type: application/json' -d "$(jq -n --arg c "$code" '{code: $c}')" "http://$address/v1/session" | jq -r .token) ||
    { check "page: no browser session from $address"; return 1; }
  curl -sSN -H "Authorization: Bearer $session" -H 'cozy-project-storage: 3' -H 'Accept: text/event-stream' "http://$address/v1/events" > "$OUT/page.sse" 2>&1 & PAGE_PID=$!
  sleep 2
  kill -0 "$PAGE_PID" 2> /dev/null || { check "page: its events stream did not stay open: $(tail -c 300 "$OUT/page.sse")"; return 1; }
}
close_page() { [ -z "${PAGE_PID:-}" ] || kill "$PAGE_PID" 2> /dev/null || true; PAGE_PID=''; }

# cl-962: a cozy of another version never restarts a Calcifer someone uses. With a page's stream open, the candidate CLI asks the
# release's Calcifer what it lacks (`machine use`, cl-873's route): it says "Calcifer here is <v>, this cozy is <w>" and exits, and
# that Calcifer (its pid) and the page's stream go on. A release that serves the route answers it; Calcifer still keeps its pid.
busy_skew() {
  [ -f "$CAND_COZY" ] || return 0
  step "version skew: the candidate CLI leaves a Calcifer with an open page running"
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder pid code=0 release candidate
  pid=$(calcifer_record pid) release=$(cozy --version) candidate=$("$CAND_COZY" --version)
  open_page || return
  (cd "$OUT/project" && "$CAND_COZY" machine use local) > "$OUT/skew-busy.txt" 2>&1 || code=$?
  cat "$OUT/skew-busy.txt"
  if [ "$code" != 0 ]; then
    grep -qF "Calcifer here is ${release#cozy }, this cozy is ${candidate#cozy }" "$OUT/skew-busy.txt" && grep -q 'cozy restart' "$OUT/skew-busy.txt" \
      || check "skew: the candidate CLI's refused ask did not say which versions meet and \`cozy restart\`"
  fi
  [ "$(jq -r .pid "$holder")" = "$pid" ] || check "skew: the candidate CLI restarted the release's Calcifer under an open page (pid $pid is now $(jq -r .pid "$holder"))"
  kill -0 "$PAGE_PID" 2> /dev/null || check "skew: the page's events stream was cut"
  ! grep -q 'restarting it' "$OUT/skew-busy.txt" || check "skew: the candidate CLI said it restarts Calcifer"
  close_page
}

# cl-962: the installed cozy that finds a Calcifer of another version (an install swapped under it; here its lock's record says
# 0.0.0-skew) keeps it running while a page's stream is open, and says so. A real skew is busy_skew's; this one runs wherever the
# installed cozy is the candidate (isolated CI).
kept_calcifer() {
  step "the installed cozy keeps a Calcifer of another version that a page uses"
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder pid code=0
  pid=$(calcifer_record pid)
  open_page || return
  cp "$holder" "$OUT/holder.saved"
  jq -c '.version = "0.0.0-skew"' "$OUT/holder.saved" > "$holder.skew" && mv -f "$holder.skew" "$holder"
  cozy machine ls > "$OUT/kept.txt" 2>&1 || code=$?
  cp "$OUT/holder.saved" "$holder"
  cat "$OUT/kept.txt"
  [ "$code" = 0 ] || check "kept: cozy machine ls failed against a Calcifer of another version (exit $code)"
  grep -q 'Calcifer here is 0.0.0-skew, this cozy is .*: it keeps running (a page is open' "$OUT/kept.txt" \
    || check "kept: the installed cozy did not say it keeps a Calcifer a page uses"
  ! grep -q 'restarting it' "$OUT/kept.txt" || check "kept: the installed cozy said it restarts Calcifer"
  [ "$(calcifer_record pid)" = "$pid" ] || check "kept: Calcifer was restarted under an open page (pid $pid is now $(calcifer_record pid))"
  kill -0 "$PAGE_PID" 2> /dev/null || check "kept: the page's events stream was cut"
  close_page
}

# cl-962: `cozy restart` is the one way to switch a Calcifer in use: with a page open, Calcifer comes back on the installed cozy
# under a new pid, the page's stream ends (its page reconnects), and the next command is served.
explicit_restart() {
  step "cozy restart"
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder pid
  pid=$(calcifer_record pid)
  open_page || return
  cozy --json restart > "$OUT/restart.json" 2> "$OUT/restart.err" || { check "cozy restart failed: $(tail -c 300 "$OUT/restart.err")"; close_page; return; }
  J --argjson old "$pid" '.restarted and .pid != $old' "$OUT/restart.json" || check "cozy restart: $(cat "$OUT/restart.json")"
  [ "$(jq -r .pid "$holder")" = "$(jq -r .pid "$OUT/restart.json")" ] || check "cozy restart: the lock names pid $(jq -r .pid "$holder")"
  sleep 2
  ! kill -0 "$PAGE_PID" 2> /dev/null || check "cozy restart: the old Calcifer's page stream is still open"
  close_page
  cozy --json machine ls > /dev/null || check "cozy restart: the next command was not served"
}

# slow_upload <bytes>: a PUT /v1/files of that many random bytes at 200 KB/s to the running Calcifer, in the background ($UP),
# its status in $OUT/slow.code.
slow_upload() {
  local digest address token
  address=$(calcifer_record address) token=$(calcifer_record token)
  head -c "$1" /dev/urandom > "$OUT/slow.bin"
  digest=sha256:$(sha "$OUT/slow.bin")
  curl -sS -o "$OUT/slow.json" -w '%{http_code}' --limit-rate 200k -T "$OUT/slow.bin" -H "Authorization: Bearer $token" \
    "http://$address/v1/files/$digest" > "$OUT/slow.code" 2> "$OUT/slow.err" & UP=$!
  sleep 2
}

# cl-962: the laptop's dev-local.sh restarts Calcifer through scripts/restart-calcifer-when-idle.py once it serves nothing: an
# upload in flight holds the restart until it lands, and past its bound it restarts anyway and says so. Linux, where Calcifer is
# the systemd user unit cozy-calcifer.
idle_restart() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder restart=$E2E/../scripts/restart-calcifer-when-idle.py pid code t0
  step "a Calcifer restart waits for an upload in flight"
  pid=$(calcifer_record pid) t0=$SECONDS
  slow_upload 3000000
  python3 "$restart" --wait 120 --every 1 > "$OUT/idle-restart.txt" 2>&1 || check "idle restart failed: $(tail -c 300 "$OUT/idle-restart.txt")"
  wait "$UP" || true
  code=$(cat "$OUT/slow.code")
  cat "$OUT/idle-restart.txt"
  { grep -q 'restart waits: [0-9]* request' "$OUT/idle-restart.txt" && grep -q 'restarted idle' "$OUT/idle-restart.txt"; } \
    || check "idle restart: it did not wait for the upload in flight"
  [ "$code" = 201 ] || [ "$code" = 200 ] || check "idle restart: the upload in flight was cut ($code: $(tail -c 300 "$OUT/slow.err" "$OUT/slow.json"))"
  [ $((SECONDS - t0)) -ge 10 ] || check "idle restart: Calcifer restarted after $((SECONDS - t0))s, before a 15 s upload landed"
  cozy --json machine ls > /dev/null || check "idle restart: the next command was not served"
  [ "$(calcifer_record pid)" != "$pid" ] || check "idle restart: Calcifer kept pid $pid"
  step "past its bound a Calcifer restart goes ahead and says so"
  pid=$(calcifer_record pid)
  slow_upload 6000000
  python3 "$restart" --wait 3 --every 1 > "$OUT/idle-bound.txt" 2>&1 || check "bounded restart failed: $(tail -c 300 "$OUT/idle-bound.txt")"
  wait "$UP" || true
  cat "$OUT/idle-bound.txt"
  grep -q 'restarted while busy after [0-9]*s, the most it waits: [0-9]* request' "$OUT/idle-bound.txt" || check "bounded restart: it did not say it restarted busy"
  cozy --json machine ls > /dev/null || check "bounded restart: the next command was not served"
  [ "$(calcifer_record pid)" != "$pid" ] || check "bounded restart: Calcifer kept pid $pid"
}

sha() { { sha256sum "$1" 2> /dev/null || shasum -a 256 "$1"; } | cut -c1-64; }
J() { jq -e "$@" > /dev/null; }
# boxes_in <folder>: stdin's JSON array of boxes, each a `POST /v1/projects/{id}/assets` body ({name, tool, kind, fields, at}), made in
# order through Calcifer as the page makes them, each exactly named; the folder made a project first unless it is one.
boxes_in() {
  local boxes box
  boxes=$(jq -c '.[] | {tool: "", exact: true} + .') && (run_project "$1") || { check "boxes in $1: no project, or not a JSON array of boxes"; return 1; }
  while IFS= read -r box; do
    [ -n "$box" ] || continue
    project_api "$1" /assets -X POST -H 'Content-Type: application/json' -d "$box" > "$OUT/boxes-in.txt" 2>&1 \
      || { check "box $(jq -r .name <<< "$box") in $1: $(tail -c 300 "$OUT/boxes-in.txt")"; return 1; }
  done <<< "$boxes"
}
# db_sql <project> <sql>: statements run on its project.db past Calcifer, as another program would.
db_sql() {
  python3 - "$1/project.db" "$2" <<'PY'
import sqlite3, sys
conn = sqlite3.connect(sys.argv[1], isolation_level=None, timeout=30)
conn.executescript(sys.argv[2])
PY
}
# owed <project>: how many runs its walk has pending.
owed() { sql1 "$1" "SELECT count(*) FROM walk, json_each(walk.record, '\$.pending')"; }
# api <curl args…>: Calcifer, with the bearer of its lock record, on project storage protocol 3.
api() { curl -sS -H "Authorization: Bearer $TOKEN" -H 'cozy-project-storage: 3' "$@"; }
# status <project>: what status says. Why it can't be read goes to the log, and an answer the CLI can't read is a failure of its own.
status() { project_api "$1" '' 2> "$OUT/status.err"; }
# row <project> <asset> <jq>: that of its row in status.
row() { status "$1" | jq -c --arg n "$2" ".assets[] | select(.name == \$n) | $3"; }
# made <project> <asset>: how many versions the lock keeps.
made() { local n; n=$(held "$1" "$2" 2> /dev/null | jq '.versions | length'); echo "${n:-0}"; }
# held <project> <asset>: what `cozy status <asset>` says of it, `.asset` and `.versions`. Why it can't goes to the log.
held() { project_api "$1" "/assets/$2" 2> "$OUT/held.err"; }

# The project and page cells' fixtures are tools (`@app.tool`, cozy-runtime >=0.43). A machine with no Runtime of its own takes PyPI's
# newest, which before 0.43 is published refuses them: said once, here, and the cells don't go on.
tools_read() {
step "a project: the fixtures' tools read on this machine"
(cd "$OUT/project" && in_project "$OUT/project" run "$here/project/draw.py/draw" instructions=read --on local) > "$OUT/tools-read.txt" 2>&1 \
  || fail "the fixtures' tools don't read here: the machine's Runtime (${CAND_RUNTIME:-none of its own, so the newest on PyPI}) needs cozy-runtime's tools (rc/tools), as --runtime: $(tail -c 300 "$OUT/tools-read.txt")"
}

# Renders queued from one shell, seen, one canceled, and the rest land. A canceled `render all` doesn't send again what it canceled.
# The walk (cl-770): every job known when asked, each going on its own, a deleted asset's queued job saying so, and nothing of the walk left behind.
walking() {
step "a project: what's queued is known at once, a long job holds up nothing it isn't read by, a deleted asset's job stops and lands nothing"
local W=$OUT/walking stream=$OUT/walking-events.txt id run back
mkdir -p "$W"
jq -n --arg shots "$here/project/shots.py/slow" --arg draw "$here/project/draw.py/draw" '[
  {name: "running_gone", tool: $shots, kind: "video", fields: {seconds: 900}},
  {name: "reads_gone", tool: $draw, kind: "image", fields: {instructions: "From <running_gone>, which is deleted as it runs."}},
  {name: "slow", tool: $shots, kind: "video", fields: {seconds: 5}},
  {name: "beside", tool: $draw, kind: "image", fields: {instructions: "Reads nothing, so it goes now."}},
  {name: "one", tool: $draw, kind: "image", fields: {instructions: "From <slow>."}},
  {name: "two", tool: $draw, kind: "image", fields: {instructions: "From <slow>."}},
  {name: "last", tool: $draw, kind: "image", fields: {instructions: "From <one> and <two>."}},
  {name: "gone_soon", tool: $draw, kind: "image", fields: {instructions: "Also from <slow>, and deleted while it waits."}}]' | boxes_in "$W" || return
id=$(status "$W" | jq -r .id)
api -N --max-time 120 "http://$ADDR/v1/events" > "$stream" 2> /dev/null &
local tail=$!
show_events "$stream" "$id" || check "walking: project interest not held"
api -f -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: walking-$RANDOM$RANDOM" \
  -d '{"names":["all"],"on":["local"]}' "http://$ADDR/v1/projects/$id/runs" > "$OUT/walking-queue.txt" 2>&1 || check "walking: render: $(tail -c 300 "$OUT/walking-queue.txt")"
J '[.board.jobs[].asset] as $a | (["running_gone", "reads_gone", "slow", "beside", "one", "two", "last", "gone_soon"] - $a) == []' <<< "$(status "$W")" || check "walking: not every job is on the board at once: $(status "$W" | jq -c '[.board.jobs[] | .asset]')"
J '.board.jobs[] | select(.asset == "last") | .needs | contains(["one", "two"])' <<< "$(status "$W")" || check "walking: last doesn't say what it waits for: $(status "$W" | jq -c '.board.jobs[] | select(.asset == "last")')"
for _ in $(seq 240); do run=$(status "$W" | jq -r '.board.jobs[] | select(.asset == "running_gone") | .run // empty'); [ -n "$run" ] && [ "$(in_project "$W" --json run watch "$run" --once | jq -r .status)" = in_progress ] && break; sleep 0.25; done
[ "$(in_project "$W" --json run watch "$run" --once | jq -r .status)" = in_progress ] || check "walking: running_gone isn't running: $(status "$W" | jq -c '.board.jobs[] | select(.asset == "running_gone")')"
# On one machine they queue there: each went as soon as it could, not after the long job.
for _ in $(seq 120); do J '[.board.jobs[] | select(.asset == "slow" or .asset == "beside") | .run] | all' <<< "$(status "$W")" && break; sleep 0.25; done
J '[.board.jobs[] | select(.asset == "slow" or .asset == "beside") | .run] | all' <<< "$(status "$W")" || check "walking: what reads nothing waited for the long job: $(status "$W" | jq -c '[.board.jobs[] | {asset, run}]')"
[ "$(owed "$W")" -ge 1 ] || check "walking: what the walk owes isn't in project.db"
project_api "$W" /assets/gone_soon -X DELETE > /dev/null || check "walking: deleting gone_soon failed"
for _ in $(seq 20); do J '.board.jobs[] | select(.asset == "gone_soon") | .state == "canceled"' <<< "$(status "$W")" && break; sleep 0.25; done
api -o /dev/null -X DELETE "http://$ADDR/v1/projects/$id/assets/running_gone"
back=$(api "http://$ADDR/v1/projects/$id/history" | jq -r '[.entries[] | select(.undone == null and (.names | index("running_gone")))][0].id')
for _ in $(seq 60); do [ "$(in_project "$W" --json run watch "$run" --once | jq -r .status)" = canceled ] && break; sleep 0.5; done
[ "$(in_project "$W" --json run watch "$run" --once | jq -r .status)" = canceled ] || check "walking: a deleted asset's run goes on billing: $(in_project "$W" --json run watch "$run" --once | jq -c '{status, progress}')"
for _ in $(seq 240); do J '.busy | not' <<< "$(status "$W")" && break; sleep 0.5; done
soon told "$stream" ".type == \"board.updated\" and .project == \"$id\" and .board.ended != null" || check "walking: terminal board not delivered"
kill "$tail" 2> /dev/null
{ [ "$(made "$W" slow)" = 1 ] && [ "$(made "$W" beside)" = 1 ] && [ "$(made "$W" one)" = 1 ] && [ "$(made "$W" two)" = 1 ] && [ "$(made "$W" last)" = 1 ]; } || check "walking: made $(status "$W" | jq -c '[.assets[] | {(.name): .versions}] | add')"
for gone in gone_soon running_gone; do
  [ "$(made "$W" "$gone")" = 0 ] || check "walking: deleted $gone has a version"
  J --arg gone "$gone" '.board.jobs[] | select(.asset == $gone) | .state == "canceled" and (.why | test("deleted"))' <<< "$(status "$W")" || check "walking: deleted $gone's job says $(status "$W" | jq -c --arg gone "$gone" '.board.jobs[] | select(.asset == $gone)')"
done
J '.board.jobs[] | select(.asset == "reads_gone") | .state == "failed" and (.error.message | test("running_gone was deleted"))' <<< "$(status "$W")" || check "walking: what read a deleted asset says $(status "$W" | jq -c '.board.jobs[] | select(.asset == "reads_gone")')"
# Its delete undone, it is the asset, and not the render it stopped.
J --argjson e "$back" '[.entries[].id] == [$e]' <<< "$(api -X POST -d "{\"entry\": $back}" "http://$ADDR/v1/projects/$id/history/undo")" || check "walking: undo didn't take back the DELETE (entry $back)"
{ [ -n "$(row "$W" running_gone .name)" ] && J '.busy | not' <<< "$(status "$W")" && [ "$(made "$W" running_gone)" = 0 ]; } || check "walking: put back, running_gone went again: $(status "$W" | jq -c '{busy, jobs: [.board.jobs[] | select(.asset == "running_gone")]}')"
[ "$(owed "$W")" = 0 ] || check "walking: what the walk owed outlived the render"
[ -z "$(find "$W/.runs" -type f 2> /dev/null)" ] || check "walking: left in .runs: $(find "$W/.runs" -type f)"
grep -q "\"type\":\"job.updated\".*\"project\":\"$id\"\|\"project\":\"$id\".*\"type\":\"job.updated\"" "$stream" || check "walking: /v1/events told no job.updated: $(head -c 300 "$stream")"
# Its end is the board's, told whole on the same stream, as status keeps it.
sse "$stream" | J -s --arg id "$id" 'map(select(.type == "board.updated" and .project == $id)) | (length > 1) and (last.board.ended != null) and (first.board.ended == null)' \
  || check "walking: /v1/events told no board.updated that began and ended it: $(sse "$stream" | jq -c --arg id "$id" 'select(.type == "board.updated" and .project == $id) | {seq: .board.seq, ended: .board.ended}' | tail -3)"
J '.board.ended != null and .board.elapsed_ms != null' <<< "$(status "$W")" || check "walking: status's board doesn't say it ended: $(status "$W" | jq -c '.board | {elapsed_ms, ended}')"
}


# sse <file>: what a stream wrote, one JSON object a line.
sse() { sed -n 's/^data: //p' "$1"; }
# revs <file> <id>: each `rev` the stream told of that project.
revs() { sse "$1" | jq -r --arg i "$2" 'select(.type == "project.changed" and .id == $i) | .rev'; }
# soon <command…>: it comes true within 20 s.
soon() { for _ in $(seq 100); do "$@" && return 0; sleep 0.2; done; return 1; }
told() { sse "$1" | jq -e "select($2)" > /dev/null; }
told_more() { [ "$(revs "$1" "$2" | wc -l)" -gt "$3" ]; }
show_events() {
  local log=$1 project=$2 stream
  soon told "$log" '.type == "hello"' || return
  stream=$(sse "$log" | sed -n '1p' | jq -er 'select(.type == "hello") | .stream') || return
  api -f -X PUT -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$project" '{projects: [$id]}')" \
    "http://$ADDR/v1/events/$stream" | J --arg id "$project" '.held | index($id) != null'
}

project_stream() {
  local holder=$HOME/.cozy/calcifer/calcifer.lock.holder ADDR TOKEN
  ADDR=$(jq -er .address "$holder") && TOKEN=$(jq -er .token "$holder") || fail "project stream: missing Calcifer session"
  walking
  event_stream
}

# `GET /v1/events`, read as the page reads it: the first event, an edit from outside, a landing that writes only the lock, the local
# machine, a comment when nothing happens, and the routes it replaced gone.
event_stream() {
step "a project: GET /v1/events"
local R=$OUT/stream log=$OUT/events.sse other=$OUT/events-unrelated.sse stream other_stream id code state n seen boxes
starter_project "$R" > /dev/null || check "stream: project new"
state=$(status "$R") || fail "stream: project state unavailable"
id=$(jq -er .id <<< "$state") && code=$(jq -er .code <<< "$state") || fail "stream: project identity or link code unavailable"
curl -sSN -H "Authorization: Bearer $TOKEN" -H 'cozy-project-storage: 3' "http://$ADDR/v1/events" > "$log" 2> "$OUT/events.err" & stream=$!
# Register with the public link code; deliveries still use the stable project ID.
show_events "$log" "$code" || check "stream: project interest not held: $(head -c 300 "$log"; cat "$OUT/events.err")"
curl -sSN -H "Authorization: Bearer $TOKEN" -H 'cozy-project-storage: 3' "http://$ADDR/v1/events" > "$other" 2> "$OUT/events-unrelated.err" & other_stream=$!
soon told "$other" '.type == "hello"' || check "stream: no hello on unrelated connection"
sse "$log" | sed -n 1p | J --arg v "$(fixture_calcifer | jq -r .calcifer.version)" '.type == "hello" and .version == $v' || check "stream: its first event: $(sse "$log" | sed -n 1p)"

seen=$(revs "$log" "$id" | wc -l)
# An edit from outside: a box another client adds.
project_api "$R" /assets -X POST -H 'Content-Type: application/json' -d '{"name": "edited", "exact": true, "tool": "", "fields": {"text": "By hand."}}' > /dev/null \
  || check "stream: another client's edit refused"
soon told_more "$log" "$id" "$seen" || check "stream: another client's edit told nothing: $(sse "$log" | cut -c1-200 | tail -5)"

seen=$(revs "$log" "$id" | wc -l) boxes=$(status "$R" | jq -cS '[.assets[] | {name, tool, fields}]')
(cd "$R" && in_project "$R" run "$here/project/draw.py" instructions=stream) > "$OUT/stream-run.txt" 2>&1 || check "stream: a run in the project: $(tail -c 300 "$OUT/stream-run.txt")"
n=$(cd "$R" && in_project "$R" --json run ls --limit 1 | jq -r '.runs[0].number') || check "stream: run ls in the project failed"
[ "$(status "$R" | jq -cS '[.assets[] | {name, tool, fields}]')" = "$boxes" ] || check "stream: a landing changed the boxes, and this was to prove its own rows reach the stream"
soon told_more "$log" "$id" "$seen" || check "stream: a landing's lock told nothing"
soon told "$log" ".type == \"run.updated\" and .run.number == $n and .run.status == \"completed\" and .run.project_id == \"$id\"" \
  || check "stream: no run.updated says run $n completed in $id: $(sse "$log" | jq -c 'select(.type == "run.updated") | [.run.number, .run.status, .run.project_id]' | tail -5)"
soon told "$other" ".type == \"run.updated\" and .run.number == $n and .run.status == \"completed\"" || check "stream: a connection with no projects missed the global run"
# Another program's change (xs-189: Calcifer watches the folder, it no longer looks): a file leaving files/, as a sync tool's would,
# and coming back.
local kept aside=$OUT/stream-aside
kept=$(find "$R/files" -type f | head -1)
seen=$(revs "$log" "$id" | wc -l)
{ [ -n "$kept" ] && mv "$kept" "$aside"; } || check "stream: the landing left nothing in files/"
soon told_more "$log" "$id" "$seen" || check "stream: a file leaving files/ told nothing"
seen=$(revs "$log" "$id" | wc -l)
[ -z "$kept" ] || mv "$aside" "$kept"
soon told_more "$log" "$id" "$seen" || check "stream: a file back in files/ told nothing"
[ -z "$(revs "$other" "$id")" ] || check "stream: project changes reached a connection with no interest"
soon told "$log" '.type == "machine.updated" and .machine.name == "local"' || check "stream: a run on local moved no machine.updated"
[ -z "$(revs "$log" "$id" | sort | uniq -d)" ] || check "stream: a rev told twice: $(revs "$log" "$id" | sort | uniq -d)"

soon grep -q '^:' "$log" || check "stream: no comment after 20 s"
kill -0 "$stream" 2> /dev/null || check "stream: it ended: $(cat "$OUT/events.err")"
kill "$stream" "$other_stream" 2> /dev/null || true
for route in runs machines "projects/$id"; do
  [ "$(api -o /dev/null -w '%{http_code}' "http://$ADDR/v1/$route/events")" = 404 ] || check "stream: /v1/$route/events still answers"
done
}

# running <screen>: what the local machine's row of `cozy machine ls` says under RUNNING.
running() { awk '!at { at = index($0, "RUNNING") } at && $1 == "local" { print substr($0, at, 7) + 0; exit }' "$1"; }
screen_says() { grep -qE "$2" "$1" 2> /dev/null; }
running_above() { [ "$(running "$1")" -gt "$2" ] 2> /dev/null; }
running_back() { [ "$(running "$1")" -le "$2" ] 2> /dev/null; }

# `cozy run ls` and `cozy machine ls` on a terminal: a change reaches them, and (when Calcifer here is this suite's) across a restart.
live_views() {
step "a project: the live views follow the stream"
local R=$OUT/live term n before code follower
starter_project "$R" > /dev/null || check "live: project new"
python3 "$here/project/terminal.py" "$OUT/runs.screen" "$COZY" run ls --all > /dev/null & term=$!
soon screen_says "$OUT/runs.screen" 'TARGET' || check "live: cozy run ls drew nothing"
(cd "$R" && in_project "$R" run "$here/project/draw.py" instructions=live) > "$OUT/live-run.txt" 2>&1 || check "live: a run: $(tail -c 300 "$OUT/live-run.txt")"
n=$(cd "$R" && in_project "$R" --json run ls --limit 1 | jq -r '.runs[0].number') || check "live: run ls in the project failed"
soon screen_says "$OUT/runs.screen" "^ *$n .* completed" || check "live: cozy run ls does not show run $n completed: $(head -5 "$OUT/runs.screen")"

if [ -z "${ALONE:-}" ] && [ "$RUNNER_OS" = Linux ]; then
  step "a project: a live view across a Calcifer restart, the branch resync takes"
  systemctl --user restart cozy-calcifer || check "live: systemctl --user restart cozy-calcifer failed"
  (cd "$R" && in_project "$R" run "$here/project/draw.py" instructions=restarted) > "$OUT/live-restarted.txt" 2>&1 || check "live: a run after the restart: $(tail -c 300 "$OUT/live-restarted.txt")"
  n=$(cd "$R" && in_project "$R" --json run ls --limit 1 | jq -r '.runs[0].number') || check "live: run ls after the restart failed"
  soon screen_says "$OUT/runs.screen" "^ *$n .* completed" || check "live: past a restart, cozy run ls does not show run $n: $(head -5 "$OUT/runs.screen")"
  ADDR=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder") TOKEN=$(jq -r .token "$HOME/.cozy/calcifer/calcifer.lock.holder")
fi
code=0; kill "$term" 2> /dev/null || true; wait "$term" || code=$?
[ "$code" = 0 ] || check "live: cozy run ls exited $code on q"

python3 "$here/project/terminal.py" "$OUT/machines.screen" "$COZY" machine ls > /dev/null & term=$!
soon screen_says "$OUT/machines.screen" '^ *local ' || check "live: cozy machine ls shows no local: $(head -5 "$OUT/machines.screen")"
before=$(running "$OUT/machines.screen")
(cd "$here" && in_project "$R" run "$here/weightless/wait" seconds:=8 --on local > "$OUT/live-wait.txt" 2>&1) & follower=$!
# Its first run here may install first: it's judged while its follower lives.
until running_above "$OUT/machines.screen" "$before" || ! kill -0 "$follower" 2> /dev/null; do sleep 0.2; done
running_above "$OUT/machines.screen" "$before" || check "live: cozy machine ls never showed the run on local: $(head -5 "$OUT/machines.screen")"
wait "$follower" || check "live: the run on local failed: $(tail -c 300 "$OUT/live-wait.txt")"
soon running_back "$OUT/machines.screen" "$before" || check "live: cozy machine ls still shows the ended run: $(head -5 "$OUT/machines.screen")"
code=0; kill "$term" 2> /dev/null || true; wait "$term" || code=$?
[ "$code" = 0 ] || check "live: cozy machine ls exited $code on q"
}

# The standalone CLI check covers the retained project and run entrypoints.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  . "$here/../lib.sh"
  parse "$@"
  python3 "$here/entrypoints.py" --cozy "$COZY" --out "$OUT/entrypoints"
fi