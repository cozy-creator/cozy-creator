# e2e/cli/statedb.sh: Calcifer's own state is in state.db (cl-818), each cell through the CLI and Calcifer's HTTP, no machine or model.
#   state_crash    Calcifer killed (kill -9) amid project opens and tool-list edits: each whole or absent, each one answered kept
#   state_files    nothing in ~/.cozy/calcifer is a JSON store (blobs by hash stay files), state.db is its owner's alone, and it keeps no
#                  browser session (cl-985)
#   state_import   OLD=<dir>, an earlier Calcifer's files as its build wrote them: imported at the next start, then gone; its sessions
#                  deleted unread
#   state_restart  Calcifer restarted on 40,000 runs and 3,000 ended rentals, the first read of each page of state.db slow as on a busy
#                  disk's cold cache (busy-disk.c): every route a client asks answers in 200 ms while it takes up what it left, and
#                  none is said slow in health.log (cl-1010)
# Uses project.sh's `api` and projectdb.sh's `kill9`, `db_calcifer` and `calcifer_halt`.

# state <query>: what state.db holds, read only, one JSON row a line.
state() {
  python3 - "$HOME/.cozy/calcifer/state.db" "$1" <<'PY'
import json, sqlite3, sys
conn = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
for row in conn.execute(sys.argv[2]):
    print(json.dumps(list(row)))
PY
}
state1() { state "$1" | head -1 | jq -r '.[0] // empty'; }

statedb() {
state_crash
state_files
}

state_crash() {
step "state.db: Calcifer killed amid project opens and tool-list edits: each whole or absent, each one answered kept"
local S=$OUT/state-crash n=60 many=3000 i w1 w2 opened edited id path
mkdir -p "$S"
db_calcifer
# Each its own transaction: a project made and known; a tool off your list.
open_one() { api -sS -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: state-open-$1" -d "{\"path\": \"$S/p$1\", \"unencrypted\": true}" "http://$ADDR/v1/projects"; }
edit_one() { api -sS -X POST -H 'Content-Type: application/json' -d "{\"remove\": [\"qwen-image-2/crash_$1\"]}" "http://$ADDR/v1/tools/list"; }
: > "$S/opened" && : > "$S/edited"
(for ((i = 1; i <= n; i++)); do open_one "$i" 2> /dev/null | jq -ce 'select(.id) | {id, path}' >> "$S/opened" || true; done) & w1=$!
(for ((i = 1; i <= many; i++)); do edit_one "$i" 2> /dev/null | jq -ce --arg t "qwen-image-2/crash_$i" 'select(.without | index($t)) | $t' >> "$S/edited" || true; done) & w2=$!
until { [ "$(wc -l < "$S/opened")" -ge 15 ] && [ "$(wc -l < "$S/edited")" -ge 15 ]; } || ! kill -0 "$w1" 2> /dev/null || ! kill -0 "$w2" 2> /dev/null; do sleep 0.05; done
kill9
kill "$w1" "$w2" 2> /dev/null || true
wait "$w1" "$w2" 2> /dev/null || true
opened=$(wc -l < "$S/opened") edited=$(wc -l < "$S/edited")
echo "state crash: killed with $opened of $n opens and $edited of $many tool edits answered"
{ [ "$opened" -lt "$n" ] && [ "$edited" -lt "$many" ]; } || check "state crash: Calcifer was killed after one kind was all answered: not amid both"
# As the kill left it, before any Calcifer opens it again.
[ "$(state1 'PRAGMA integrity_check')" = ok ] || check "state crash: integrity: $(state 'PRAGMA integrity_check')"
local torn=() lost=() rows
# Whole: a project known is a folder holding its project.db.
while read -r id path; do
  [ -f "$path/project.db" ] || torn+=("$id at $path holds no project.db")
done < <(state "SELECT id, path FROM projects WHERE path LIKE '$S/%'" | jq -r '"\(.[0]) \(.[1])"')
while read -r id path; do
  [ "$(state1 "SELECT path FROM projects WHERE id = '$id'")" = "$path" ] || lost+=("project $id")
done < <(jq -r '"\(.id) \(.path)"' "$S/opened")
# Taking tools off adds no package: your list starts empty (cl-896).
rows=$(state 'SELECT package FROM tool_list ORDER BY ord' | jq -sc 'map(.[0])')
[ "$rows" = '[]' ] || torn+=("your list's packages $rows")
while read -r tool; do
  [ "$(state1 "SELECT count(*) FROM tools_off WHERE tool = '$tool'")" = 1 ] || lost+=("tool $tool")
done < <(jq -r . "$S/edited")
[ ${#torn[@]} = 0 ] || check "state crash: torn: ${torn[*]:0:5}"
[ ${#lost[@]} = 0 ] || check "state crash: answered, then lost: ${lost[*]:0:5}"
echo "state crash: in state.db $(state1 "SELECT count(*) FROM projects WHERE path LIKE '$S/%'") projects, $(state1 "SELECT count(*) FROM tools_off WHERE tool LIKE 'qwen-image-2/crash_%'") tools off"
# The Calcifer the next command starts answers each as it was kept.
cozy --json machine ls > /dev/null 2> "$S/ls.err" || check "state crash: no Calcifer after the kill: $(tail -c 300 "$S/ls.err")"
db_calcifer
api "http://$ADDR/v1/projects?limit=1000" > "$S/ls.json"
lost=()
while read -r id path; do
  J --arg i "$id" --arg p "$path" '.projects | any(.id == $i and .path == $p and (.missing | not))' "$S/ls.json" || lost+=("project $id")
done < <(jq -r '"\(.id) \(.path)"' "$S/opened")
api -sS -X POST -H 'Content-Type: application/json' -d '{}' "http://$ADDR/v1/tools/list" > "$S/list.json"
while read -r tool; do J --arg t "$tool" '.without | index($t)' "$S/list.json" || lost+=("tool $tool"); done < <(jq -r . "$S/edited")
[ ${#lost[@]} = 0 ] || check "state crash: after the restart, not answered as kept: ${lost[*]:0:5}"
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(printf '0%.0s' {1..64})" "http://$ADDR/v1/status")" = 401 ] || check "state crash: a bearer not the lock record's admits"
# As it was: the tools back on.
api -sS -o /dev/null -X POST -H 'Content-Type: application/json' -d '{"add": "qwen-image-2"}' "http://$ADDR/v1/tools/list" || true
}

state_files() {
step "state.db: no JSON store in ~/.cozy/calcifer, state.db its owner's alone, and no browser session kept"
local dir=$HOME/.cozy/calcifer stores table
# Blobs stay files, each named by its sha256 (`files/`, `local/`'s manifests): a store is a file of what changes.
stores=$(cd "$dir" && find . -maxdepth 1 -name '*.json' ! -name refused.json | sort | paste -sd' ')
[ -z "$stores" ] || check "state: JSON stores in $dir: $stores"
[ ! -e "$dir/after" ] || check "state: $dir/after is still there: $(ls -A "$dir/after" | head -5 | paste -sd' ')"
[ "$(stat -c %a "$dir/state.db")" = 600 ] || check "state: state.db is $(stat -c %a "$dir/state.db"), not 600"
for table in projects tool_list tools_off rendered_here headers after_steps; do
  [ "$(state1 "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '$table'")" = 1 ] || check "state: state.db has no $table"
done
# Nothing signs a page in (cl-985): a Calcifer that kept sessions has them dropped at its next start.
for table in sessions session_codes; do
  [ "$(state1 "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '$table'")" = 0 ] || check "state: state.db still has $table"
done
# What a project's page has not seen is the project's own, in its project.db: state.db holds none.
[ "$(state1 "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'unseen'")" = 0 ] || check "state: state.db holds a global unseen table"
echo "state: $dir holds $(cd "$dir" && ls -A | paste -sd' ')"
}

# OLD=<dir>: copies of an earlier Calcifer's projects.json, sessions.json, tools.json, here.json, checkpoints.json and after/, as its build
# wrote them. With Calcifer stopped they go where it kept them; its next start imports each and deletes it, sessions.json unread.
state_import() {
step "state.db: an earlier Calcifer's files imported at the first start, then gone"
local dir=$HOME/.cozy/calcifer lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid name
[ -d "${OLD:-}" ] || { check "state import: OLD names no folder of an earlier Calcifer's files"; return; }
# Stopped, if it runs: SIGTERM, as a service manager stops it.
if curl -fsS -o /dev/null -H "Authorization: Bearer $(jq -r .token "$lock" 2> /dev/null)" "http://$(jq -r .address "$lock" 2> /dev/null)/v1/status" 2> /dev/null; then
  pid=$(jq -r .pid < "$lock")
  kill -TERM "$pid"
  while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done
fi
for name in projects.json sessions.json tools.json here.json checkpoints.json after; do
  [ ! -e "$OLD/$name" ] || cp -a "$OLD/$name" "$dir/"
done
cozy --json machine ls > /dev/null 2> "$OUT/state-import-ls.err" || check "state import: Calcifer didn't start: $(tail -c 300 "$OUT/state-import-ls.err")"
db_calcifer
api "http://$ADDR/v1/projects?limit=1000" > "$OUT/state-import-ls.json"
for name in projects.json sessions.json tools.json here.json checkpoints.json after; do
  [ ! -e "$dir/$name" ] || check "state import: $name is still there"
done
local want got key
if [ -f "$OLD/projects.json" ]; then
  # Each as projects.json had it: a row here already (a later open) wins.
  want=$(jq -c '[.projects | to_entries[] | {id: .key, path: .value.path}] | sort_by(.id)' "$OLD/projects.json")
  got=$(jq -c --argjson want "$want" '[.projects[] | {id, path} | select(IN($want[]))] | sort_by(.id)' "$OUT/state-import-ls.json")
  [ "$want" = "$got" ] || check "state import: projects $got, not $want"
  echo "state import: $(jq length <<< "$got") projects, as projects.json had them; walking: $(state "SELECT id FROM projects WHERE walking" | jq -sc 'map(.[0])')"
fi
if [ -f "$OLD/tools.json" ]; then
  want=$(jq -c '{packages, without: (.without // [])}' "$OLD/tools.json")
  got=$(api -sS -X POST -H 'Content-Type: application/json' -d '{}' "http://$ADDR/v1/tools/list" | jq -c '{packages, without: (.without // [])}')
  [ "$want" = "$got" ] || check "state import: your list $got, not $want"
  echo "state import: your list $got"
fi
if [ -f "$OLD/sessions.json" ]; then
  # Nothing of it kept (state_files: no sessions table): none of its digests is in state.db, and a token signed in before the cut, if
  # the run that made OLD kept them, is refused.
  local kept=0 digest token
  while read -r digest; do
    ! grep -qsaF "$digest" "$dir/state.db" "$dir/state.db-wal" || kept=$((kept + 1))
  done < <(jq -r '.[] | (.token, .media) | strings' "$OLD/sessions.json")
  [ "$kept" = 0 ] || check "state import: $kept of sessions.json's digests are in state.db"
  [ ! -f "$OLD/tokens" ] || while read -r token; do
    [ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "http://$ADDR/v1/status")" = 401 ] || check "state import: a session from before the cut admits"
  done < "$OLD/tokens"
  echo "state import: sessions.json's $(jq length "$OLD/sessions.json") sessions deleted, none kept"
fi
if [ -f "$OLD/here.json" ]; then
  want=$(jq -c '[.here[] | [.tool, .machine, (.bytes // 0)]] | sort' "$OLD/here.json")
  got=$(state 'SELECT tool, machine, bytes FROM rendered_here' | jq -sc sort)
  [ "$want" = "$got" ] || check "state import: Render here $got, not $want"
  echo "state import: Render here $got"
fi
if [ -f "$OLD/checkpoints.json" ]; then
  want=$(jq -c '[.facts | to_entries[] | [(.key | split(" ") | .[0]), (.key | split(" ") | .[1]), .value.lora, .value.family]] | sort' "$OLD/checkpoints.json")
  got=$(state 'SELECT hub, sha256, lora = 1, family FROM headers' | jq -sc 'map(.[2] |= (. == 1)) | sort')
  [ "$want" = "$got" ] || check "state import: headers $(jq length <<< "$got"), not $(jq length <<< "$want")"
  echo "state import: $(jq length <<< "$got") headers"
fi
if [ -d "$OLD/after" ]; then
  # Each a row until its run settles, which does it and forgets it.
  want=$(cd "$OLD/after" && for f in *.json; do [ -e "$f" ] && printf '%s\n' "${f%.json}"; done | sort | paste -sd' ')
  got=$(state 'SELECT key_sha256 FROM after_steps' | jq -r '.[0]' | sort | paste -sd' ')
  for key in $got; do [[ " $want " == *" $key "* ]] || check "state import: a step $key no file had"; done
  echo "state import: steps in after/ ${want:-none}; in state.db now ${got:-none}"
fi
state_files
}

state_restart() {
[ "$RUNNER_OS" = Linux ] && [ -n "$LOCAL" ] || return 0
step "state.db: a restart on 40,000 runs, each page's first read slow: every route answers in 200 ms (cl-1010)"
local S=$OUT/state-restart db=$HOME/.cozy/calcifer/state.db holder=$HOME/.cozy/calcifer/calcifer.lock.holder
local dropin=$HOME/.config/systemd/user/cozy-calcifer.service.d/cl1010-busy-disk.conf id pid
mkdir -p "$S"
cc -std=c11 -O2 -Wall -Wextra -Werror -shared -fPIC -o "$S/busy-disk.so" "$here/busy-disk.c" -ldl || { check "restart: busy-disk.c didn't build"; return; }
cozy --json machine ls > /dev/null && db_calcifer && id=$(id_of "$RUN_PROJECT") && [ -n "$id" ] || { check "restart: no project to ask about"; return; }
# Its tools listed once, as a computer's page has: the list a restart answers from is kept.
api -fsS -o /dev/null "http://$ADDR/v1/tools" || check "restart: GET /v1/tools before the restart failed"
pid=$(jq -r .pid "$holder")
calcifer_halt || return
python3 "$here/restart_store.py" fill "$db" "$HOME/.cozy/outputs/e2e-restart" --runs 40000 --machines 3000 > "$S/fill.json" 2>&1 \
  || { check "restart: the store wasn't filled: $(tail -c 300 "$S/fill.json")"; systemctl --user start cozy-calcifer; return; }
echo "restart: store $(cat "$S/fill.json")"
mkdir -p "${dropin%/*}" && printf '[Service]\nEnvironment=LD_PRELOAD=%s\nEnvironment=BUSY_DISK_LOG=%s\n' "$S/busy-disk.so" "$S/busy-disk.log" > "$dropin"
systemctl --user daemon-reload && systemctl --user start cozy-calcifer || check "restart: systemctl --user start cozy-calcifer failed"
python3 "$here/restart_store.py" measure --holder "$holder" --old-pid "$pid" --project "$id" --log "$HOME/.cozy/calcifer/health.log" \
  --timed "$S/timed.json" --seconds 6 --budget-ms 200 > "$S/latency.ndjson" 2>&1 \
  || check "restart: routes over 200 ms, or said slow, as Calcifer restarted on 40,000 runs: $(tail -n 1 "$S/latency.ndjson" | jq -c '{over, slow: (.slow_requests // [] | .[:3]), steps}' 2> /dev/null || tail -c 600 "$S/latency.ndjson")"
cat "$S/latency.ndjson"
[ -s "$S/busy-disk.log" ] || check "restart: no read of state.db was slowed: the cell proves nothing"
echo "restart: reads slowed $(tail -1 "$S/busy-disk.log" 2> /dev/null)"
# Kept 90 days (Paul 11-15): a new run of no project, here the candidates again, takes out 16 runs and 16 ended machines past them.
local cut old_runs old_machines runs_after machines_after
cut="(CAST(strftime('%s', 'now') AS INTEGER) - 90 * 86400) * 1000"
old_runs=$(state1 "SELECT count(*) FROM runs WHERE state = 'settled' AND settled_at < $cut")
old_machines=$(state1 "SELECT count(*) FROM machines WHERE state = 'ended' AND coalesce(ended_at, created_at) < $cut")
if [ -n "${CAND_RT:-}${CAND_TFS:-}" ]; then
  cozy --json machine update local ${CAND_TFS:+--tensorfs "$CAND_TFS"} ${CAND_RT:+--runtime "$CAND_RT"} -y > "$S/update.json" 2>&1 || check "restart: the candidates again: $(tail -c 300 "$S/update.json")"
  runs_after=$(state1 "SELECT count(*) FROM runs WHERE state = 'settled' AND settled_at < $cut")
  machines_after=$(state1 "SELECT count(*) FROM machines WHERE state = 'ended' AND coalesce(ended_at, created_at) < $cut")
  echo "restart: past 90 days, runs $old_runs → $runs_after, ended machines $old_machines → $machines_after"
  [ "$((old_runs - runs_after))" -ge 16 ] || check "restart: a new run took out $((old_runs - runs_after)) runs past 90 days, not 16"
  [ "$((old_machines - machines_after))" -ge 16 ] || check "restart: a new run took out $((old_machines - machines_after)) ended machines past 90 days, not 16"
fi
# The store as it was for what follows: its rows out, and Calcifer on its own disk again.
rm -f "$dropin" && systemctl --user daemon-reload
calcifer_halt || return
python3 "$here/restart_store.py" empty "$db" || check "restart: the store's rows weren't taken out"
cozy --json machine ls > /dev/null || check "restart: Calcifer didn't start again"
}
