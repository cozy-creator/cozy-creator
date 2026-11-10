# e2e/cli/statedb.sh: Calcifer's own state is in state.db (cl-818), each cell through the CLI and Calcifer's HTTP, no machine or model.
#   state_crash    Calcifer killed (kill -9) amid project opens, tool-list edits and sign-ins: each whole or absent, each one answered kept
#   state_files    nothing in ~/.cozy/calcifer is a JSON store (blobs by hash stay files), and state.db is its owner's alone
#   state_import   OLD=<dir>, an earlier Calcifer's files as its build wrote them: imported at the next start, then gone
# Uses project.sh's `api` and projectdb.sh's `kill9` and `db_calcifer`.

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
digest() { printf %s "$1" | sha256sum | cut -c1-64; }
# The tools Your list starts with.
DEFAULT_TOOLS='["_/qwen-image-2","_/minimax-h3"]'

statedb() {
state_crash
state_files
}

state_crash() {
step "state.db: Calcifer killed amid project opens, tool-list edits and sign-ins: each whole or absent, each one answered kept"
local S=$OUT/state-crash n=60 many=3000 i w1 w2 w3 opened edited signed id path token
mkdir -p "$S"
db_calcifer
# Each its own transaction: a project made and known; a tool off your list; a session from a code.
open_one() { api -sS -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: state-open-$1" -d "{\"path\": \"$S/p$1\"}" "http://$ADDR/v1/projects"; }
edit_one() { api -sS -X POST -H 'Content-Type: application/json' -d "{\"remove\": [\"qwen-image-2/crash_$1\"]}" "http://$ADDR/v1/tools/list"; }
sign_one() {
  local code
  code=$(api -sS -X POST "http://$ADDR/v1/session-codes" | jq -r '.code // empty') && [ -n "$code" ] || return 1
  curl -sS -X POST -H 'Content-Type: application/json' -d "{\"code\": \"$code\"}" "http://$ADDR/v1/session"
}
: > "$S/opened" && : > "$S/edited" && : > "$S/signed"
(for ((i = 1; i <= n; i++)); do open_one "$i" 2> /dev/null | jq -ce 'select(.id) | {id, path}' >> "$S/opened" || true; done) & w1=$!
(for ((i = 1; i <= many; i++)); do edit_one "$i" 2> /dev/null | jq -ce --arg t "qwen-image-2/crash_$i" 'select(.without | index($t)) | $t' >> "$S/edited" || true; done) & w2=$!
(for ((i = 1; i <= many; i++)); do sign_one "$i" 2> /dev/null | jq -ce 'select(.token)' >> "$S/signed" || true; done) & w3=$!
until { [ "$(wc -l < "$S/opened")" -ge 15 ] && [ "$(wc -l < "$S/edited")" -ge 15 ] && [ "$(wc -l < "$S/signed")" -ge 15 ]; } \
  || ! kill -0 "$w1" 2> /dev/null || ! kill -0 "$w2" 2> /dev/null || ! kill -0 "$w3" 2> /dev/null; do sleep 0.05; done
kill9
kill "$w1" "$w2" "$w3" 2> /dev/null || true
wait "$w1" "$w2" "$w3" 2> /dev/null || true
opened=$(wc -l < "$S/opened") edited=$(wc -l < "$S/edited") signed=$(wc -l < "$S/signed")
echo "state crash: killed with $opened of $n opens, $edited of $many tool edits and $signed of $many sign-ins answered"
{ [ "$opened" -lt "$n" ] && [ "$edited" -lt "$many" ] && [ "$signed" -lt "$many" ]; } || check "state crash: Calcifer was killed after one kind was all answered: not amid all three"
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
rows=$(state 'SELECT package FROM tool_list ORDER BY ord' | jq -sc 'map(.[0])')
[ "$rows" = "$DEFAULT_TOOLS" ] || torn+=("your list's packages $rows")
while read -r tool; do
  [ "$(state1 "SELECT count(*) FROM tools_off WHERE tool = '$tool'")" = 1 ] || lost+=("tool $tool")
done < <(jq -r . "$S/edited")
while read -r token; do
  [ "$(state1 "SELECT count(*) FROM sessions WHERE token_sha256 = '$(digest "$token")'")" = 1 ] || lost+=("session ${token:0:8}")
done < <(jq -r .token "$S/signed")
[ ${#torn[@]} = 0 ] || check "state crash: torn: ${torn[*]:0:5}"
[ ${#lost[@]} = 0 ] || check "state crash: answered, then lost: ${lost[*]:0:5}"
echo "state crash: in state.db $(state1 "SELECT count(*) FROM projects WHERE path LIKE '$S/%'") projects, $(state1 "SELECT count(*) FROM tools_off WHERE tool LIKE 'qwen-image-2/crash_%'") tools off, $(state1 'SELECT count(*) FROM sessions') sessions"
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
while read -r token; do
  [ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "http://$ADDR/v1/status")" = 200 ] || lost+=("session ${token:0:8}")
done < <(jq -r .token "$S/signed")
[ ${#lost[@]} = 0 ] || check "state crash: after the restart, not answered as kept: ${lost[*]:0:5}"
[ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(printf '0%.0s' {1..64})" "http://$ADDR/v1/status")" = 401 ] || check "state crash: a token never given admits"
# As it was: the tools back on, the sessions ended.
api -sS -o /dev/null -X POST -H 'Content-Type: application/json' -d '{"add": "qwen-image-2"}' "http://$ADDR/v1/tools/list" || true
while read -r token; do curl -sS -o /dev/null -X DELETE -H "Authorization: Bearer $token" "http://$ADDR/v1/session" || true; done < <(jq -r .token "$S/signed")
}

state_files() {
step "state.db: no JSON store in ~/.cozy/calcifer, and state.db its owner's alone"
local dir=$HOME/.cozy/calcifer stores table
# Blobs stay files, each named by its sha256 (`files/`, `local/`'s manifests): a store is a file of what changes.
stores=$(cd "$dir" && find . -maxdepth 1 -name '*.json' ! -name refused.json | sort | paste -sd' ')
[ -z "$stores" ] || check "state: JSON stores in $dir: $stores"
[ ! -e "$dir/after" ] || check "state: $dir/after is still there: $(ls -A "$dir/after" | head -5 | paste -sd' ')"
[ "$(stat -c %a "$dir/state.db")" = 600 ] || check "state: state.db is $(stat -c %a "$dir/state.db"), not 600"
for table in projects sessions tool_list tools_off rendered_here headers after_steps; do
  [ "$(state1 "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '$table'")" = 1 ] || check "state: state.db has no $table"
done
echo "state: $dir holds $(cd "$dir" && ls -A | paste -sd' ')"
}

# OLD=<dir>: copies of an earlier Calcifer's projects.json, sessions.json, tools.json, here.json, checkpoints.json and after/, as its build
# wrote them. With Calcifer stopped they go where it kept them; its next start imports each and deletes it.
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
  want=$(jq -c '[.[] | [.token, .media]] | sort' "$OLD/sessions.json")
  got=$(state 'SELECT token_sha256, media_sha256 FROM sessions' | jq -sc sort)
  [ "$want" = "$got" ] || check "state import: sessions $got, not $want"
  echo "state import: $(jq length <<< "$got") sessions"
  # A token signed in before the cut still admits, if the run that made OLD kept it.
  [ ! -f "$OLD/tokens" ] || while read -r token; do
    [ "$(curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "http://$ADDR/v1/status")" = 200 ] || check "state import: a session from before the cut doesn't admit"
  done < "$OLD/tokens"
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
