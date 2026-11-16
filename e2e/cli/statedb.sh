# e2e/cli/statedb.sh: Calcifer's own state is in state.db (cl-818), each cell through the CLI and Calcifer's HTTP, no machine or model.
#   state_crash    Calcifer killed (kill -9) amid project opens and tool-list edits: each whole or absent, each one answered kept
#   state_files    nothing in ~/.cozy/calcifer is a JSON store (blobs by hash stay files), state.db is its owner's alone, and it keeps no
#                  browser session (cl-985)
#   state_import   OLD=<dir>, an earlier Calcifer's files as its build wrote them: imported at the next start, then gone; its sessions
#                  deleted unread
#   state_restart  Calcifer restarted on 40,000 runs, 3,000 ended rentals and four 3,000-box projects (opened by nothing at the start),
#                  the first read of each page of state.db slow as on a busy disk's cold cache (busy-disk.c), PyPI and the Hub 3 s
#                  late (slow_proxy.py): every route a client asks answers in 200 ms while it takes up what it left, and none is
#                  said slow in health.log, nor the start's purge (cl-1010)
#   vault_purge_due a deleted box's next purge is in state.db for a plaintext project only: an encrypted one's never, unlocked or
#                  locked, and it's still purged once its window ended and it's unlocked again (cl-1010)
# Uses project.sh's `api` and projectdb.sh's `kill9`, `db_calcifer`, `calcifer_halt` and `undo_window`.

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
for table in projects tool_list tools_off headers after_steps; do
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
# Four projects of a real size (3,000 boxes each; Paul's largest project.db is 27 MB), with nothing deleted: a start opens none of them
# (a start's purge opened every known project and loaded its document, 5 s on Paul's laptop, and their requests waited on it).
local large=() big i
for i in 1 2 3 4; do
  big=$S/large-$i
  cozy --json new --unencrypted "$big" > "$S/large-$i-new.json" 2>&1 || { check "restart: project $i: $(tail -c 300 "$S/large-$i-new.json")"; continue; }
  python3 - "$big/nodes.yaml" 3000 "$i" <<'PY'
import sys
with open(sys.argv[1], "w") as out:
    for n in range(int(sys.argv[2])):
        out.write(f"note_{n}:\n  kind: text\n  text: Note {n} of project {sys.argv[3]}. The colossal blue whale rises through a storm, lit from below.\n")
PY
  in_project "$big" apply -f nodes.yaml > "$S/large-$i-apply.txt" 2>&1 || { check "restart: project $i's 3,000 boxes: $(tail -c 300 "$S/large-$i-apply.txt")"; continue; }
  large+=("$(id_of "$big")")
done
# Its tools listed once, as a computer's page has: the list a restart answers from is kept.
api -fsS -o /dev/null "http://$ADDR/v1/tools" || check "restart: GET /v1/tools before the restart failed"
# A dev home, as Paul's laptop is: a packages checkout (`dev = true`, `[calcifer] packages`) whose first-party package a list reads
# from source with uv (from_source), `local` held on its candidates by hand so following the RC moves nothing (source.sh does so too).
local co=$S/packages cfg=$HOME/.cozy/config.toml dev=
case $("$COZY" --version) in *-dev+g*) dev=1 ;; esac
if [ -n "$dev" ]; then
  rm -rf "$co" && mkdir -p "$co"
  cp -r "$here/../../release-gate/suites/hub/pkg" "$co/minimax-h3-tools" && mv "$co/minimax-h3-tools/ci_hub_pkg" "$co/minimax-h3-tools/minimax_h3_tools"
  sed -i -e 's/^name = "ci-hub-pkg"/name = "minimax-h3-tools"/' -e 's/ci_hub_pkg/minimax_h3_tools/g' "$co/minimax-h3-tools/pyproject.toml" "$co/minimax-h3-tools/package.toml"
  git -C "$co" init -q -b rc/e2e && git -C "$co" add -A && git -C "$co" -c user.name=e2e -c user.email=e2e@localhost commit -qm checkout \
    || check "restart: the checkout's commit failed"
  [ -z "${CAND_RT:-}" ] || cozy --json machine update local ${CAND_TFS:+--tensorfs "$CAND_TFS"} --runtime "$CAND_RT" -y > "$S/hold.json" 2>&1 \
    || check "restart: local refused its candidates by hand: $(tail -c 300 "$S/hold.json")"
  cp "$cfg" "$S/config.toml"
  { echo 'dev = true'; grep -v '^dev *=' "$S/config.toml"; } > "$cfg.next"
  if grep -q '^\[calcifer\]' "$cfg.next"; then sed -i "/^\[calcifer\]/a packages = \"$co\"" "$cfg.next"; else printf '\n[calcifer]\npackages = "%s"\n' "$co" >> "$cfg.next"; fi
  mv "$cfg.next" "$cfg"
fi
pid=$(jq -r .pid "$holder")
calcifer_halt || return
python3 "$here/restart_store.py" fill "$db" "$HOME/.cozy/outputs/e2e-restart" --runs 40000 --machines 3000 > "$S/fill.json" 2>&1 \
  || { check "restart: the store wasn't filled: $(tail -c 300 "$S/fill.json")"; systemctl --user start cozy-calcifer; return; }
echo "restart: store $(cat "$S/fill.json")"
# A laptop's network: PyPI and the Hub answer each request 3 s late (slow_proxy.py), so a route that waits on them can't pass here.
local proxy proxied
python3 "$here/slow_proxy.py" "$S/proxy.port" 3 2> "$S/proxy.log" & proxy=$!
for _ in $(seq 50); do [ -s "$S/proxy.port" ] && break; sleep 0.1; done
proxied=http://127.0.0.1:$(cat "$S/proxy.port")
mkdir -p "${dropin%/*}" && printf '[Service]\nEnvironment=LD_PRELOAD=%s\nEnvironment=BUSY_DISK_LOG=%s\nEnvironment=HTTP_PROXY=%s\nEnvironment=HTTPS_PROXY=%s\nEnvironment=http_proxy=%s\nEnvironment=https_proxy=%s\n' \
  "$S/busy-disk.so" "$S/busy-disk.log" "$proxied" "$proxied" "$proxied" "$proxied" > "$dropin"
systemctl --user daemon-reload && systemctl --user start cozy-calcifer || check "restart: systemctl --user start cozy-calcifer failed"
python3 "$here/restart_store.py" measure --holder "$holder" --old-pid "$pid" --project "$id" --log "$HOME/.cozy/calcifer/health.log" \
  --run "$(jq -r '.unsettled[0]' "$S/fill.json")" --large "$(IFS=,; echo "${large[*]}")" \
  --timed "$S/timed.json" --seconds 6 --budget-ms 200 > "$S/latency.ndjson" 2>&1 \
  || check "restart: routes over 200 ms, or said slow, as Calcifer restarted on 40,000 runs: $(tail -n 1 "$S/latency.ndjson" | jq -c '{over, slow: (.slow_requests // [] | .[:3]), steps}' 2> /dev/null || tail -c 600 "$S/latency.ndjson")"
cat "$S/latency.ndjson"
[ -s "$S/busy-disk.log" ] || check "restart: no read of state.db was slowed: the cell proves nothing"
# Resume took up the runs an earlier Calcifer left going: each settles, as the machine has none of them.
local going
for _ in $(seq 120); do going=$(state1 "SELECT count(*) FROM runs WHERE id LIKE 'run-cl1010u%' AND state <> 'settled'") && [ "$going" = 0 ] && break; sleep 0.5; done
[ "$going" = 0 ] || check "restart: $going of the runs left going weren't taken up 60 s after the restart"
echo "restart: reads slowed $(tail -1 "$S/busy-disk.log" 2> /dev/null); network asks delayed $(grep -c . "$S/proxy.log" 2> /dev/null || echo 0)"
# Ended machines are kept 90 days (Paul 11-15): those older went as Calcifer took up what it left; the rest stayed.
local cut gone kept
cut="(CAST(strftime('%s', 'now') AS INTEGER) - 90 * 86400) * 1000"
local unended
gone=$(state1 "SELECT count(*) FROM machines WHERE name LIKE 'cl1010-gone-%' AND ended_at < $cut")
kept=$(state1 "SELECT count(*) FROM machines WHERE name LIKE 'cl1010-gone-%'")
unended=$(state1 "SELECT count(*) FROM machines WHERE name LIKE 'cl1010-unended-%'")
echo "restart: ended machines past 90 days left $gone, of $kept kept; $unended of 20 with no end time kept"
[ "$gone" = 0 ] || check "restart: $gone ended machines past 90 days are still kept"
[ "$kept" -ge 1400 ] || check "restart: only $kept of the ended machines within 90 days are kept"
# Ended before their end was kept, made 101 days ago: their 90 days count from this start (cl-1010, #3424's review).
[ "$unended" = 20 ] || check "restart: $((20 - unended)) machines that ended before their end was kept went at once, counted from when they were made"
# The store as it was for what follows: its rows out, and Calcifer on its own disk and config again.
rm -f "$dropin" && systemctl --user daemon-reload
[ -z "$dev" ] || cp "$S/config.toml" "$cfg"
kill "$proxy" 2> /dev/null || true
calcifer_halt || return
python3 "$here/restart_store.py" empty "$db" || check "restart: the store's rows weren't taken out"
cozy --json machine ls > /dev/null || check "restart: Calcifer didn't start again"
}

# cl-1010: a start opens only the projects whose deleted boxes are due, by the times state.db keeps (`purge_due`). Of a locked encrypted
# project Calcifer keeps only its folder (Paul's no-traces rule): a row saying something was deleted in it, and when, is kept for none.
# An encrypted project's due time is held only while it's unlocked, and its unlock's open reads it again from its own project.db.
vault_purge_due() (
  step "a deleted box's next purge: in state.db for a plaintext project, never for an encrypted one; purged after its unlock (cl-1010)"
  [ "${RUNNER_OS:-Linux}" = Linux ] || { echo "vault purge due: Linux runners only"; exit 0; }
  local work secrets P plain view id plain_id pic deleted_at rows lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  work=$(mktemp -d "$OUT/vault-purge-due.XXXXXX") secrets=$(mktemp -d "$OUT/vault-purge-due-credentials.XXXXXX")
  P=$work/encrypted plain=$work/plain
  trap 'in_project "$P" --json vault lock > /dev/null 2>&1 || true; undo_window "$lock" ""; rm -rf -- "$secrets"' EXIT
  printf '%s\n' 'synthetic purge-due passphrase' > "$secrets/pass" && chmod 600 "$secrets/pass"
  # due_rows <id>: how many purge_due rows state.db holds of that project.
  due_rows() { state "SELECT count(*) FROM purge_due WHERE project = '$1'" | jq -r '.[0]'; }
  # locked <name>: the project locked, its admitted work (a thumbnail, a measure) drained first if a lock found it busy.
  locked() {
    in_project "$P" --json vault lock > "$work/$1.json" && return 0
    jq -e '.error.code == "project_busy"' "$work/$1.json" > /dev/null || return 1
    for _ in $(seq 80); do
      in_project "$P" --json vault status | jq -e '.state == "locked"' > /dev/null && return 0
      sleep 0.25
    done
    return 1
  }
  python3 -c '
import struct, sys, zlib
def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
raw = b"".join(b"\x00" + bytes((200, 30, 30)) * 64 for _ in range(48))
head = struct.pack(">IIBBBBB", 64, 48, 8, 2, 0, 0, 0)
open(sys.argv[1], "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", head) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))
' "$work/red.png" || fail "vault purge due: no picture"
  # A short undo window before any project opens: Calcifer restarts for it, which would lock an open encrypted project.
  undo_window "$lock" 20
  step "vault purge due: a plaintext project's deleted box is due in state.db"
  cozy --json new --unencrypted "$plain" > "$work/plain-new.json" 2>&1 || fail "vault purge due: new plaintext project: $(tail -c 300 "$work/plain-new.json")"
  project_import "$plain" "$work/red.png" pic > /dev/null && project_api "$plain" /assets/pic -X DELETE > /dev/null \
    || fail "vault purge due: the plaintext project's box wasn't made and deleted"
  plain_id=$(id_of "$plain")
  rows=$(due_rows "$plain_id")
  [ "$rows" = 1 ] || check "vault purge due: the plaintext project's deleted box isn't due in state.db ($rows rows)"
  step "vault purge due: an encrypted project's deleted box is due nowhere outside it, unlocked or locked"
  cozy --json new "$P" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$work/new.json" 2> "$work/new.err" \
    || fail "vault purge due: new encrypted project: $(tail -c 300 "$work/new.err")"
  view=$(jq -r '.view // empty' "$work/new.json")
  [ -n "$view" ] && [ -d "$view" ] || fail "vault purge due: the encrypted project has no unlocked view: $(head -c 300 "$work/new.json")"
  project_import "$P" "$work/red.png" pic > /dev/null || fail "vault purge due: the encrypted project's picture wasn't imported"
  id=$(id_of "$P") pic=$(asset_of "$P" pic | jq -r '.sha256 // empty')
  [ -n "$id" ] && [ -n "$pic" ] && in_files "$view" "$pic" || fail "vault purge due: the encrypted project's picture isn't in its files/"
  project_api "$P" /assets/pic -X DELETE > /dev/null || fail "vault purge due: the encrypted project's box wasn't deleted"
  deleted_at=$(date +%s)
  rows=$(due_rows "$id")
  [ "$rows" = 0 ] || check "vault purge due: state.db holds the unlocked encrypted project's next purge ($rows rows)"
  locked lock || fail "vault purge due: lock: $(head -c 300 "$work/lock.json")"
  rows=$(due_rows "$id")
  [ "$rows" = 0 ] || check "vault purge due: state.db holds the locked encrypted project's next purge ($rows rows)"
  # Its window ends while it's locked: nothing can read it then.
  while [ $(($(date +%s) - deleted_at)) -le 22 ]; do sleep 1; done
  rows=$(due_rows "$id")
  [ "$rows" = 0 ] || check "vault purge due: past its window, state.db holds the locked encrypted project's next purge ($rows rows)"
  step "vault purge due: unlocked past its window, the deleted box's file goes"
  in_project "$P" --json vault unlock --passphrase-file "$secrets/pass" --idle 0 > "$work/unlock.json" 2> "$work/unlock.err" \
    || fail "vault purge due: unlock: $(head -c 300 "$work/unlock.json") $(tail -c 200 "$work/unlock.err")"
  view=$(jq -r '.vault.view // empty' "$work/unlock.json")
  [ -n "$view" ] && [ -d "$view" ] || fail "vault purge due: unlocked with no view: $(head -c 300 "$work/unlock.json")"
  for _ in $(seq 150); do in_files "$view" "$pic" || break; sleep 0.2; done
  ! in_files "$view" "$pic" || check "vault purge due: unlocked past its window, the encrypted project's deleted box kept its file"
  rows=$(due_rows "$id")
  [ "$rows" = 0 ] || check "vault purge due: after its purge, state.db holds the encrypted project's next purge ($rows rows)"
  locked relock || check "vault purge due: lock after the purge: $(head -c 300 "$work/relock.json")"
  trap - EXIT
  undo_window "$lock" ""
  rm -rf -- "$secrets" "$P"
  drop_folder "$plain"
)
