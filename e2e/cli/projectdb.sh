# e2e/cli/projectdb.sh: a project's state is one SQLite database, project.db (cl-803), each cell through the CLI and Calcifer's HTTP
# as the page calls it, no machine or model needed.
#   db_first_open   current creation/reopen and stable undo identity; old physical project formats are refused
#   db_crash        Calcifer killed (kill -9) amid writes of a box, its row and its push-aside: each whole or absent; a resend is done once
#   db_crdt         the native CRDT: stable identities, old-schema refusal, projection damage, replica merges, append and restart
#   db_latency      write latency at a project with 500 takes (it runs on the cozy before project.db too: --shard projectdb-latency)
#   db_migrate      a project 0.24.0 made opens here with nothing lost: its takes, files and words, ids and history (schema 3)
# Uses project.sh's `api`, `status`, `held` and `sha`.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# sql <project> <query>: what project.db holds, read only, one JSON row a line.
sql() {
  python3 - "$1/project.db" "$2" <<'PY'
import json, sqlite3, sys
conn = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
for row in conn.execute(sys.argv[2]):
    print(json.dumps(list(row)))
PY
}
# sql1 <project> <query>: its first row's first value, plain.
sql1() { sql "$1" "$2" | head -1 | jq -r '.[0] // empty'; }
db_calcifer() { local lock=$HOME/.cozy/calcifer/calcifer.lock.holder; ADDR=$(jq -r .address "$lock") TOKEN=$(jq -r .token "$lock"); }
id_of() { status "$1" | jq -r .id; }
# kill9 Calcifer by its lock's pid, and wait for it gone.
kill9() {
  local pid; pid=$(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")
  kill -9 "$pid" 2> /dev/null || true
  while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done
}
tiny() { base64 -d > "$1" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='; }

# The cells project.db changes most: its first open and a crash, and the page's tools, versions, nodes, place, undo, live, layout,
# offline and held.
db_existing() {
tools_read
db_first_open
db_crash
db_existing_web
}

db_existing_web() {
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder pw=$OUT/playwright chrome restart name
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "web: no Google Chrome (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 || { check "web: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
restart=$(web_restart "$lock")
for name in ${DB_WEB:-place nodes versions tools live layout undo offline held}; do
  step "web-$name"
  case $name in
    instant) web_proof "$name" "$chrome" "$pw" "$restart" ;;
    palette) CATALOG=1 NO_HUB=$(web_restart "$lock" http://127.0.0.1:9) web_proof "$name" "$chrome" "$pw" "$restart" ;;
    undo | offline) STOP=$(web_halt "$lock") START=$(web_start) web_proof "$name" "$chrome" "$pw" "$restart" ;;
    held) web_held "$chrome" "$pw" ;;
    deleted) web_deleted "$chrome" "$pw" "$lock" ;;
    *) web_proof "$name" "$chrome" "$pw" '' ;;
  esac
  [ "$name" != scale ] || web_scale_said
done
}

# A new take never changes what's approved (cl-908): the page's double tap, its Render, a mention's real run (web/approvals.js).
approvals() { DB_WEB=approvals db_existing_web; }

# A rendering box at rest breathes around its edge; a take landing beside an approved one is unseen until looked at (cl-928): web/newtakes.js.
newtakes() { DB_WEB=newtakes db_existing_web; }

# The take looked at in Focus is the one drawn when it finishes, with Focus left and opened again while it's made, and from the toast's View (cl-942):
# web/focusfinish.js.
focusfinish() { DB_WEB=focusfinish db_existing_web; }

# A take is one id from Render to its version: listed at once, never off, through a slow Calcifer, a reload and a Cancel (cl-945):
# web/takeids.js.
takeids() { DB_WEB=takeids db_existing_web; }

db_cancel_undo() {
[ -n "$LOCAL" ] || return
[ -z "${ALONE:-}" ] || { check "cancel/undo proof requires an owned fresh-runner Calcifer"; return; }
step "project: a delete held at commit cancels its render, and undo retains its three prior versions"
local P=$OUT/cancel-undo tool=$here/project/tool.py/slow
cozy --json new --unencrypted "$P" > "$OUT/cancel-new.json" 2> "$OUT/cancel-new.err" || { check "cancel/undo: new"; return; }
in_project "$P" --json run "$tool" 'seconds:=0.01' 'instructions=Warm up before cancellation.' --on local \
  > "$OUT/cancel-warm.ndjson" 2> "$OUT/cancel-warm.err" || { check "cancel/undo: local source warmup"; return; }
python3 "$here/cancel_undo.py" "$P" "$OUT" "$tool" > "$OUT/cancel-undo-proof.json" 2> "$OUT/cancel-undo-proof.err" \
  || check "cancel/undo: $(cat "$OUT/cancel-undo-proof.json") $(tail -c 500 "$OUT/cancel-undo-proof.err")"
cat "$OUT/cancel-undo-proof.json"
}

# project.db's cells, through the project API and on its storage directly.
projectdb() {
db_crdt
db_cancel_undo
db_first_open
db_deleted_024
db_crash
db_latency
}

# Boxes deleted under 0.24 at the first 0.25 open (cl-919): 0.24 set a deleted box aside with no time and kept its files, and its Undo
# (the last 200 edits) brought it back with them. 0.25 dates such a delete at its first open: the files stay, Undo brings the box back,
# and once the window ends (config.toml's calcifer.undo_seconds, cut for the cell) the box left deleted loses its files. Two pictures are
# imported and deleted, then project.db is written as 0.24 left it, with Calcifer stopped: each set-aside row without `deleted_ms` or
# `held`, an imported picture's file on record as a `run 0, deleted` row.
db_deleted_024() {
step "project.db: boxes deleted under 0.24 keep their files for the undo window from the first 0.25 open; Undo brings one back"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder undo=40 a b entry name
DB024=$OUT/db-deleted-024
run_project "$DB024" > "$OUT/db-024-new.log" 2>&1 || { check "0.24 deletes: create: $(tail -c 300 "$OUT/db-024-new.log")"; return; }
python3 - "$OUT/db-024-kept.png" "$OUT/db-024-gone.png" <<'PNG'
import struct, sys, zlib
def png(path, rgb):
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    rows = (b"\x00" + bytes(rgb) * 8) * 8
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
png(sys.argv[1], (200, 40, 40)); png(sys.argv[2], (40, 40, 200))
PNG
a=$(sha "$OUT/db-024-kept.png") b=$(sha "$OUT/db-024-gone.png")
for name in kept gone; do
  project_import "$DB024" "$OUT/db-024-$name.png" "${name}_024" > "$OUT/db-024-$name-import.json" || { check "0.24 deletes: import $name: $(cat "$OUT/db-024-$name-import.json")"; return; }
done
held_024 "$a" && held_024 "$b" || { check "0.24 deletes: the imports aren't in files/"; return; }
for name in kept gone; do project_api "$DB024" "/assets/${name}_024" -X DELETE > /dev/null || { check "0.24 deletes: delete $name"; return; }; done
project_api "$DB024" /history > "$OUT/db-024-history.json" || { check "0.24 deletes: history"; return; }
entry=$(jq -r '[.entries[] | select((.names // []) | index("kept_024"))][0].id // empty' "$OUT/db-024-history.json")
[ -n "$entry" ] || { check "0.24 deletes: no history entry deletes kept_024: $(jq -c '[.entries[] | {id, words, names}]' "$OUT/db-024-history.json")"; return; }
# Stopped, project.db as 0.24 left it; started with a short window, the project not opened since.
web_undo_window "$lock" "$undo" db_024_written
[ "$(cat "$OUT/db-024-written.txt" 2> /dev/null)" = 2 ] || check "0.24 deletes: project.db not written as 0.24's: $(cat "$OUT/db-024-written.txt" "$OUT/db-024-written.err" 2> /dev/null)"
project_api "$DB024" '' > /dev/null || check "0.24 deletes: first open"
sleep 5
held_024 "$a" || check "0.24 deletes: the first 0.25 open deleted kept_024's file"
held_024 "$b" || check "0.24 deletes: the first 0.25 open deleted gone_024's file"
[ "$(sql "$DB024" "SELECT record FROM lock_assets WHERE key LIKE '%#%'" | jq -s '[.[] | .[0] | fromjson | select(.deleted_ms == null)] | length')" = 0 ] \
  || check "0.24 deletes: a set-aside row has no time after the first open"
[ "$(sql1 "$DB024" "SELECT count(*) FROM run_files WHERE run = 0 AND output = 'deleted'")" = 0 ] || check "0.24 deletes: 0.24's deleted rows are still on record"
project_history_step "$DB024" undo "$entry" > "$OUT/db-024-undo.json" || check "0.24 deletes: Undo of kept_024: $(tail -c 300 "$OUT/db-024-undo.json")"
J '.assets | any(.name == "kept_024")' <<< "$(status "$DB024")" || check "0.24 deletes: Undo didn't bring kept_024 back: $(status "$DB024" | jq -c '[.assets[].name]')"
held_024 "$a" || check "0.24 deletes: kept_024 came back without its file"
for _ in $(seq 1 $((undo + 60))); do held_024 "$b" || break; sleep 1; done
! held_024 "$b" || check "0.24 deletes: gone_024's file is still in files/ $((undo + 60)) s after the first open"
held_024 "$a" || check "0.24 deletes: the purge took kept_024's file, which a box holds again"
web_undo_window "$lock" ''
}
# held_024 <hex>: a file of that sha256 is in the cell's project's files/.
held_024() { find "$DB024/files" -type f -exec sha256sum {} + 2> /dev/null | grep -q "^$1 "; }
# project.db as 0.24 wrote a delete (Calcifer stopped): no time on a set-aside row (one holding no version's file isn't one, as 0.24 never
# set those aside), and an imported picture's file as a deleted run row.
db_024_written() {
python3 - "$DB024/project.db" > "$OUT/db-024-written.txt" 2> "$OUT/db-024-written.err" <<'SQL'
import json, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
written = 0
with db:
    last = db.execute("SELECT COALESCE(MAX(ord), 0) FROM run_files").fetchone()[0]
    for key, record in db.execute("SELECT key, record FROM lock_assets WHERE key LIKE '%#%'").fetchall():
        row = json.loads(record)
        held = row.pop("held", None)
        row.pop("deleted_ms", None)
        if [f for v in row.get("versions", []) + row.get("discarded", []) for f in (v.get("files") or {}).values()]:
            db.execute("UPDATE lock_assets SET record = ? WHERE key = ?", (json.dumps(row), key))
        else:
            db.execute("DELETE FROM lock_assets WHERE key = ?", (key,))
        if held:
            last += 1
            db.execute("INSERT INTO run_files (ord, run, function, output, sha256) VALUES (?, 0, '', 'deleted', ?)", (last, held))
        written += 1
print(written)
SQL
}

db_first_open() {
step "project.db: current creation/reopen and stable undo identities; a folder of old files is no project"
local F=$OUT/db-current before entry
run_project "$F" || { check "current open: create"; return; }
project_api "$F" /assets -X POST -H 'Content-Type: application/json' -d '{"tool":"","name":"sky","fields":{"text":"A blue sky."}}' > "$OUT/db-current-create.json" || check "current open: create text asset"
project_api "$F" /assets/sky -X PATCH -H 'Content-Type: application/json' -d '{"fields":{"text":"An evening sky."}}' > /dev/null || check "current open: edit"
project_api "$F" /assets/sky -X DELETE > /dev/null || check "current open: delete"
project_api "$F" /history > "$OUT/db-current-history.json" || check "current open: history"
entry=$(jq -r '.entries[0].id' "$OUT/db-current-history.json")
project_history_step "$F" undo "$entry" > /dev/null || check "current open: undo delete"
python3 - "$F/project.db" "$OUT/crdt-sky-before.am" <<'PYSKY'
import sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    open(sys.argv[2],'wb').write(db.execute('SELECT snapshot FROM crdt_document WHERE id=1').fetchone()[0])
PYSKY
project_history_step "$F" redo "$entry" > /dev/null || check "current open: redo delete"
project_history_step "$F" undo "$entry" > /dev/null || check "current open: repeat undo"
python3 - "$F/project.db" "$OUT/crdt-sky-after.am" <<'PYSKY'
import sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    open(sys.argv[2],'wb').write(db.execute('SELECT snapshot FROM crdt_document WHERE id=1').fetchone()[0])
PYSKY
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- --identity "$OUT/crdt-sky-before.am" "$OUT/crdt-sky-after.am" sky > "$OUT/crdt-current-identity.json" 2> "$OUT/crdt-current-identity.err" || check "current open: undo changed stable identity"
J '.assets | any(.name == "sky" and .fields.text == "An evening sky.")' <<< "$(status "$F")" || check "current open: undo lost current field values"
before=$(sql1 "$F" "SELECT value FROM meta WHERE key = 'seq'")
project_api "$F" '' > /dev/null || check "current open: reopen"
[ "$(sql1 "$F" "SELECT value FROM meta WHERE key = 'seq'")" = "$before" ] || check "current open: reopen wrote state"
# A folder of old files and no project.db is no project (cl-930): nothing reads, imports, upgrades or backs them up, and
# `cozy new` makes a project beside them. An interrupted creation's project.db.new is no project either.
local old file digest
for file in project.yaml project.lock project.canvas.yaml project.db.new; do
  old=$OUT/not-a-project-${file//./-}
  mkdir -p "$old"
  printf 'old %s\n' "$file" > "$old/$file"
  digest=$(sha "$old/$file")
  if (cd "$old" && cozy --json status) > "$old/status.json" 2> "$old/status.err"; then check "$file alone is a project"; fi
  grep -q not_in_a_project "$old/status.json" "$old/status.err" || check "$file alone: status says $(tail -c 300 "$old/status.err"), not that it's no project"
  if [ "$file" != project.db.new ]; then
    (cd "$old" && cozy --json new --unencrypted) > "$old/new.json" 2> "$old/new.err" || check "$file beside: cozy new: $(tail -c 300 "$old/new.err")"
    J '.created == true' "$old/new.json" || check "$file beside: cozy new made no project: $(cat "$old/new.json")"
    (cd "$old" && cozy --json status) > /dev/null 2> "$old/status-new.err" || check "$file beside: the new project: $(tail -c 300 "$old/status-new.err")"
  fi
  [ "$(sha "$old/$file")" = "$digest" ] && [ ! -e "$old/.cozy-legacy-v1" ] && [ ! -e "$old/$file.v2" ] && [ ! -e "$old/$file.v3" ] && [ ! -e "$old/project.db.v1" ] \
    || check "$file was changed, imported or backed up: $(ls -A "$old" | tr '\n' ' ')"
done
}

db_crash() {
step "project.db: Calcifer killed amid writes: each is whole or absent, and its resend is done once"
local K=$OUT/db-crash n=150 i seq writer answer
run_project "$K" > "$OUT/db-crash-new.txt" 2>&1 || { check "crash: native bootstrap"; return; }
db_calcifer; ID=$(id_of "$K")
seed() { api -f -o /dev/null -X POST -H 'Content-Type: application/json' -d "{\"tool\": \"\", \"name\": \"$1\", \"exact\": true, \"fields\": {\"text\": \"$2\"}}" "http://$ADDR/v1/projects/$ID/assets"; }
for ((i = 1; i <= n; i++)); do seed "m_$i" "Marker $i." && seed "r_$i" "Renamed $i." || { check "crash: seed m_$i and r_$i"; return; }; done
for ((i = 1; i <= n; i++)); do echo "\"m_$i\": {\"x\": 0, \"y\": $((i * 10))}"; done | paste -sd, | sed 's/^/{"cards": {/; s/$/}}/' > "$OUT/db-crash-rows.json"
api -X PATCH -H 'Content-Type: application/json' -d @"$OUT/db-crash-rows.json" "http://$ADDR/v1/projects/$ID/canvas" > /dev/null
seq=$(api "http://$ADDR/v1/projects/$ID/canvas" | jq .seq)
# Each: a box made with its row and marker m_i pushed aside, as a drop is; and r_i renamed s_i. One transaction each.
ask_made() { api -sS -X POST -H 'Content-Type: application/json' -H 'Cozy-Client: page e2e-tab' -H "Idempotency-Key: crash-made-$1" \
  -d "{\"tool\": \"\", \"name\": \"made_$1\", \"exact\": true, \"fields\": {\"text\": \"Made $1.\"}, \"at\": {\"x\": 5000, \"y\": $1}, \"moves\": {\"m_$1\": {\"x\": $((9000 + $1)), \"y\": 0}}, \"placed\": {\"m_$1\": $seq}}" \
  -D "$OUT/db-crash-$1.headers" -o "$OUT/db-crash-$1.json" "http://$ADDR/v1/projects/$ID/assets"; }
ask_rename() { api -sS -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: crash-rename-$1" -d "{\"from\": \"r_$1\", \"to\": \"s_$1\"}" -o /dev/null "http://$ADDR/v1/projects/$ID/rename"; }
(for ((i = 1; i <= n; i++)); do ask_made "$i" 2> /dev/null || true; ask_rename "$i" 2> /dev/null || true; done) &
writer=$!
until [ "$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 'made_%'" 2> /dev/null || echo 0)" -ge 20 ] 2> /dev/null || ! kill -0 "$writer" 2> /dev/null; do sleep 0.05; done
kill9
wait "$writer" 2> /dev/null || true
[ "$(sql1 "$K" "PRAGMA integrity_check")" = ok ] || check "crash: integrity: $(sql "$K" 'PRAGMA integrity_check')"
local made whole=0 torn=() renamed=0 split=()
made=$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 'made_%'")
for ((i = 1; i <= n; i++)); do
  local box row marker
  box=$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name = 'made_$i'")
  row=$(sql1 "$K" "SELECT x FROM canvas WHERE name = 'made_$i'")
  marker=$(sql1 "$K" "SELECT x FROM canvas WHERE name = 'm_$i'")
  case "$box/$row/$marker" in
    "1/5000.0/$((9000 + i)).0") whole=$((whole + 1)) ;;
    "0//0.0") ;;
    *) torn+=("made_$i: box $box row ${row:-none} marker ${marker:-none}") ;;
  esac
  [ "$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name IN ('r_$i', 's_$i')")" = 1 ] || split+=("r_$i")
  [ "$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name = 's_$i'")" = 0 ] || renamed=$((renamed + 1))
done
echo "crash: killed with $made boxes made, $whole whole, $renamed renamed"
[ ${#torn[@]} = 0 ] || check "crash: torn writes: ${torn[*]:0:5}"
[ ${#split[@]} = 0 ] || check "crash: a rename half done: ${split[*]:0:5}"
[ "$made" -lt "$n" ] || check "crash: Calcifer was killed only after all $n were made: nothing was mid-way"
# Everything sent again under its key, to the Calcifer the next command starts: what was done is answered as it was, the rest done
# now, each once.
in_project "$K" status > /dev/null 2>&1 || check "crash: no Calcifer after the kill"
db_calcifer
local replayed=0
for ((i = 1; i <= n; i++)); do
  ask_made "$i" || check "crash: resend made_$i got no answer"
  ! grep -qi '^idempotent-replayed: true' "$OUT/db-crash-$i.headers" || replayed=$((replayed + 1))
  ask_rename "$i" || true
done
echo "crash: resent all $n: $replayed answered as before"
[ "$replayed" -ge "$whole" ] || check "crash: $whole were done but only $replayed resends were answered as before"
[ "$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 'made_%'")" = "$n" ] || check "crash: after the resends, $(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 'made_%'") boxes, not $n"
[ "$(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 's_%'")" = "$n" ] || check "crash: after the resends, $(sql1 "$K" "SELECT count(*) FROM boxes WHERE name LIKE 's_%'") renamed, not $n"
}

# Latency: a project holding 500 takes, and each write's time through Calcifer's HTTP (curl's time_total). The same on the cozy before
# project.db, `--shard projectdb-latency` with it as the candidate, for the comparison.
db_latency() {
step "project.db: write latency at a project with 500 takes"
local L=$OUT/db-latency takes=${DB_TAKES:-500} i t began
mkdir -p "$L/clips"
starter_project "$L" > /dev/null 2>&1 || { check "latency: project new"; return; }
db_calcifer; ID=$(id_of "$L")
for ((i = 0; i < takes; i++)); do printf -v c '%06x' $((i * 33331 % 16777215)); ffmpeg -v error -y -f lavfi -i "color=c=0x$c:size=16x16:rate=24:duration=0.125" -pix_fmt yuv420p "$L/clips/$i.mp4"; done
(project_import "$L" "$L/clips/0.mp4" take > /dev/null) || { check "latency: import the first take"; return; }
# Each take held by Calcifer first, as the page sends a file, and untimed: a join names it by its sha256, no path of ours (cl-819).
local hex=()
for ((i = 1; i < takes; i++)); do
  hex[i]=$(sha "$L/clips/$i.mp4")
  api -f -o /dev/null -X PUT --data-binary "@$L/clips/$i.mp4" "http://$ADDR/v1/files/sha256:${hex[i]}" || { check "latency: PUT take $i"; return; }
done
began=$SECONDS
: > "$OUT/db-latency-join.txt"
for ((i = 1; i < takes; i++)); do
  api -o /dev/null -w '%{time_total}\n' -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: join-$RANDOM$RANDOM" -d "{\"source\": \"sha256:${hex[i]}\"}" "http://$ADDR/v1/projects/$ID/assets/take/versions" >> "$OUT/db-latency-join.txt"
done
echo "latency: $takes takes joined in $((SECONDS - began)) s"
[ "$(held "$L" take | jq '.versions | length')" = "$takes" ] || check "latency: take has $(held "$L" take | jq '.versions | length') versions, not $takes"
field() { api -o /dev/null -w '%{time_total}\n' -X PATCH -H 'Content-Type: application/json' -H "Idempotency-Key: f-$RANDOM$RANDOM" -d "{\"fields\": {\"instructions\": \"Words $1.\"}}" "http://$ADDR/v1/projects/$ID/assets/kyogre"; }
move() { api -o /dev/null -w '%{time_total}\n' -X PATCH -H 'Content-Type: application/json' -H "Idempotency-Key: m-$RANDOM$RANDOM" -d "{\"cards\": {\"groudon\": {\"x\": $1, \"y\": 5}}}" "http://$ADDR/v1/projects/$ID/canvas"; }
discard() { api -o /dev/null -w '%{time_total}\n' -X DELETE -H "Idempotency-Key: d-$RANDOM$RANDOM" "http://$ADDR/v1/projects/$ID/assets/take/versions/$((2 + $1))"; }
undo() { api -o /dev/null -w '%{time_total}\n' -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: u-$RANDOM$RANDOM" -d '{}' "http://$ADDR/v1/projects/$ID/history/undo"; }
approve() { api -o /dev/null -w '%{time_total}\n' -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: a-$RANDOM$RANDOM" -d "{\"name\": \"take\", \"version\": \"$((2 + $1))\"}" "http://$ADDR/v1/projects/$ID/approve"; }
local op
for op in field move discard undo approve; do : > "$OUT/db-latency-$op.txt"; done
for ((t = 0; t < 40; t++)); do
  field "$t" >> "$OUT/db-latency-field.txt"
  move "$t" >> "$OUT/db-latency-move.txt"
  discard "$t" >> "$OUT/db-latency-discard.txt"
  undo >> "$OUT/db-latency-undo.txt"
  approve "$t" >> "$OUT/db-latency-approve.txt"
done
python3 - "$OUT" "$(cozy --version)" "$takes" <<'PY' | tee "$OUT/db-latency.json"
import json, statistics, sys
out, version, takes = sys.argv[1], sys.argv[2], int(sys.argv[3])
said = {"cozy": version, "takes": takes}
for op in ["join", "field", "move", "discard", "undo", "approve"]:
    ms = sorted(float(line) * 1000 for line in open(f"{out}/db-latency-{op}.txt") if line.strip())
    if ms:
        said[op] = {"n": len(ms), "p50_ms": round(statistics.median(ms), 1), "p95_ms": round(ms[int(len(ms) * 0.95) - 1], 1), "max_ms": round(ms[-1], 1)}
if "join" in said:
    joins = [float(line) * 1000 for line in open(f"{out}/db-latency-join.txt") if line.strip()]
    said["join_last_100_p50_ms"] = round(statistics.median(joins[-100:]), 1)
print(json.dumps(said))
PY
}

# The persisted representation is an actual native CRDT, not a serialized JSON facade. Every
# project operation here goes through cozy; the native reader only inspects the resulting bytes.
db_crdt() {
step "project CRDT: canonical state, stable identities, old-schema refusal and native replica convergence"
local P=$OUT/crdt-project V=$OUT/crdt-v1 before after source
cozy --json new --unencrypted "$P" > "$OUT/crdt-new.json" 2> "$OUT/crdt-new.err" || { check "CRDT: project new: $(tail -c 300 "$OUT/crdt-new.err")"; return; }
J '.created == true and (.id | length > 0)' "$OUT/crdt-new.json" || check "CRDT: new did not create a project"
[ "$(sql1 "$P" 'PRAGMA user_version')" = 3 ] || { check "CRDT: project.db is not schema 3"; return; }
[ "$(sql1 "$P" 'SELECT count(*) FROM crdt_document WHERE format = 1 AND length(snapshot) > 0')" = 1 ] || check "CRDT: no native snapshot"
boxes_in "$P" <<'JSON' || { check "CRDT: seed the editable graph"; return; }
[{"name": "kyogre", "tool": "qwen-image-2/image", "kind": "image", "fields": {"instructions": "A whale."}},
 {"name": "groudon", "tool": "qwen-image-2/image", "kind": "image", "fields": {"instructions": "On shore with <kyogre>."}}]
JSON
tiny "$OUT/crdt-a.png"; cp "$OUT/crdt-a.png" "$OUT/crdt-b.png"; printf 'second' >> "$OUT/crdt-b.png"
project_import "$P" "$OUT/crdt-a.png" art > "$OUT/crdt-import.txt" 2>&1 || check "CRDT: import: $(tail -c 300 "$OUT/crdt-import.txt")"
db_calcifer
source="sha256:$(sha "$OUT/crdt-b.png")"
api --fail-with-body -X PUT --data-binary "@$OUT/crdt-b.png" "http://$ADDR/v1/files/$source" > /dev/null || check "CRDT: upload second version"
project_api "$P" /assets/art/versions -X POST -H 'Content-Type: application/json' -d "$(jq -cn --arg source "$source" '{source:$source}')" > "$OUT/crdt-version.txt" 2>&1 || check "CRDT: version: $(tail -c 300 "$OUT/crdt-version.txt")"
project_api "$P" /assets/kyogre -X PATCH -H 'Content-Type: application/json' -d '{"fields":{"instructions":"Before the rename."}}' > "$OUT/crdt-set.txt" 2>&1 || check "CRDT: set: $(tail -c 300 "$OUT/crdt-set.txt")"
python3 - "$P/project.db" "$OUT/crdt-before.am" <<'PY'
import sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    open(sys.argv[2],'wb').write(db.execute('SELECT snapshot FROM crdt_document WHERE id=1').fetchone()[0])
PY
project_api "$P" /rename -X POST -H 'Content-Type: application/json' -d '{"from":"kyogre","to":"whale"}' > "$OUT/crdt-rename.txt" 2>&1 || check "CRDT: rename: $(tail -c 300 "$OUT/crdt-rename.txt")"
python3 - "$P/project.db" "$OUT/crdt-renamed.am" "$V" <<'PY'
import os,sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    open(sys.argv[2],'wb').write(db.execute('SELECT snapshot FROM crdt_document WHERE id=1').fetchone()[0])
    os.makedirs(sys.argv[3],exist_ok=True)
    with sqlite3.connect(os.path.join(sys.argv[3],'project.db')) as old:
        db.backup(old)
        old.executescript('DROP TABLE crdt_document; DROP TABLE crdt_changes; PRAGMA user_version=1;')
PY
# This deliberately unsupported relational copy is never migrated or backed up by the product.
local old_digest; old_digest=$(sha "$V/project.db")
if (cd "$V" && cozy --json status) > "$OUT/crdt-v1-refusal.json" 2> "$OUT/crdt-v1-refusal.err"; then
  check "CRDT: relational schema 1 was accepted"
fi
grep -q project_db_version "$OUT/crdt-v1-refusal.json" "$OUT/crdt-v1-refusal.err" || check "CRDT: schema 1 did not explain current native requirement"
[ "$(sha "$V/project.db")" = "$old_digest" ] && [ "$(sql1 "$V" 'PRAGMA user_version')" = 1 ] || check "CRDT: refusal changed schema 1 data"
[ ! -e "$V/project.db.v1" ] && [ ! -e "$V/project.db.new" ] || check "CRDT: refusal made a migration backup/stage"
[ "$(sql1 "$P" 'SELECT count(*) FROM crdt_changes')" -ge 4 ] || check "CRDT: original change journal absent"
# Relational graph/history rows are now projections: damaging them must not become a logical
# project edit when the next real HTTP edit commits.
db_sql "$P" "UPDATE fields SET value = '\"QUERY_CACHE_ONLY\"' WHERE box = 'whale' AND field = 'instructions'; UPDATE ops SET words = 'QUERY_CACHE_ONLY';"
project_api "$P" /assets/groudon -X PATCH -H 'Content-Type: application/json' -d '{"fields":{"description":"A canonical-store check."}}' > "$OUT/crdt-cache-set.txt" 2>&1 || check "CRDT: write after projection damage"
project_api "$P" '' > "$OUT/crdt-cache-state.json" || check "CRDT: read after projection damage"
project_api "$P" /history > "$OUT/crdt-cache-history.json" || check "CRDT: history after projection damage"
{ ! grep -q QUERY_CACHE_ONLY "$OUT/crdt-cache-state.json" && ! grep -q QUERY_CACHE_ONLY "$OUT/crdt-cache-history.json"; } || check "CRDT: a relational projection overrode canonical state/history"
# Cargo consumes the same published native library as the product. This is a format/merge proof,
# not another daemon or a substitute project storage implementation.
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- "$OUT/crdt-before.am" "$OUT/crdt-renamed.am" > "$OUT/crdt-native.json" 2> "$OUT/crdt-native.err" \
  || check "CRDT: native format/replica proof: $(tail -c 400 "$OUT/crdt-native.err")"
J '.passed and .native_replicas == 3 and .no_files_versions_runs and (.prompt_alternatives == ["dawn","dusk","noon"])' "$OUT/crdt-native.json" \
  || check "CRDT: native merge report: $(cat "$OUT/crdt-native.json")"
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- --history-fixture "$OUT/crdt-renamed.am" "$OUT/crdt-history-merged.am" > "$OUT/crdt-history-fixture.json" 2> "$OUT/crdt-history-fixture.err" \
  || { check "CRDT: history merge fixture: $(tail -c 300 "$OUT/crdt-history-fixture.err")"; return; }
local H=$OUT/crdt-history-merged
python3 - "$P" "$H" "$OUT/crdt-history-merged.am" "$OUT/crdt-history-fixture.json" <<'PY'
import json,os,shutil,sqlite3,sys
source,target,snapshot,report=sys.argv[1:]
os.makedirs(target,exist_ok=True)
if os.path.isdir(os.path.join(source,'files')):
    shutil.copytree(os.path.join(source,'files'),os.path.join(target,'files'))
with sqlite3.connect(f'file:{source}/project.db?mode=ro',uri=True) as original:
    with sqlite3.connect(os.path.join(target,'project.db')) as db:
        original.backup(db)
        db.execute('UPDATE crdt_document SET snapshot=?, heads=? WHERE id=1',
                   (open(snapshot,'rb').read(),json.dumps(list(reversed(json.load(open(report))['heads'])))))
PY
# The product now loads two independently authored history records sharing a display number.
# Its next real edit must preserve both stable identities, rather than choose by that number.
project_api "$H" '' > "$OUT/crdt-peer-before.json" || check "CRDT: read merged native conflicts"
project_api "$H" /assets/groudon -X PATCH -H 'Content-Type: application/json' -d '{"fields":{"description":"After an offline history merge."}}' > "$OUT/crdt-history-set.txt" 2>&1 \
  || check "CRDT: write after history merge: $(tail -c 300 "$OUT/crdt-history-set.txt")"
project_api "$H" '' > "$OUT/crdt-peer-state.json" || check "CRDT: inspect merged conflict state"
J '[.warnings[]?.message | select(contains("Concurrent project values") and contains("remote dawn") and contains("remote dusk"))] | length > 0' "$OUT/crdt-peer-state.json" || check "CRDT: retained native prompt alternatives are not visible in project state"
python3 - "$OUT/crdt-peer-before.json" "$OUT/crdt-peer-state.json" <<'PY' || check "CRDT: nested conflict alternatives/order changed across API edit"
import json,sys
def conflicts(path):
    return [w['message'] for w in json.load(open(path))['warnings'] if 'Concurrent project values' in w['message']]
before,after=map(conflicts,sys.argv[1:])
assert before == after, (before,after)
for peer in ['a','b']:
    assert any(f'e2e-history-{peer}-left' in message and f'e2e-history-{peer}-right' in message and '/choices/0:' in message for message in after), after
assert any('/e2e_details:' in message for message in after), after
PY
project_api "$H" /history > "$OUT/crdt-peer-history.json" || check "CRDT: list merged peer history"
J '[.entries[] | select(.words == "e2e-history-a" or .words == "e2e-history-b")] | length == 2' "$OUT/crdt-peer-history.json" || check "CRDT: merged peer history is not readable through the API"
python3 - "$H/project.db" "$OUT/crdt-history-after.am" <<'PY'
import sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    open(sys.argv[2],'wb').write(db.execute('SELECT snapshot FROM crdt_document WHERE id=1').fetchone()[0])
PY
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- --history-check "$OUT/crdt-history-after.am" > "$OUT/crdt-history-check.json" 2> "$OUT/crdt-history-check.err" \
  || check "CRDT: history identity after next commit: $(tail -c 300 "$OUT/crdt-history-check.err")"
db_crdt_stream "$P" "$H"
}


# Project edits use Calcifer. The outside writer and corrupt copies test
# faults against data created through the surviving CLI and project API.
db_crdt_stream() {
step "project CRDT: native append, strict restart/corruption and observed-base refusal"
local P=$1 H=$2 C=$OUT/crdt-canvas-collision
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- --canvas-collision "$OUT/crdt-renamed.am" "$OUT/crdt-canvas-collision.am" > "$OUT/crdt-canvas-collision.json" 2> "$OUT/crdt-canvas-collision.err" \
  || { check "CRDT: native canvas fixture: $(tail -c 300 "$OUT/crdt-canvas-collision.err")"; return; }
CARGO_BUILD_JOBS=1 python3 - "$here" "$P" "$H" "$C" "$OUT" "$here/crdt/Cargo.toml" <<'STREAM_PROOF' > "$OUT/crdt-stream-proof.json" 2> "$OUT/crdt-stream-proof.err" || check "CRDT: stream/CAS proof: $(tail -c 600 "$OUT/crdt-stream-proof.err")"
from concurrent.futures import ThreadPoolExecutor
import json,sqlite3,subprocess,sys,time
from pathlib import Path
here,project,peer,collision,output,manifest=sys.argv[1:]
sys.path.insert(0,here)
from project_http import Refused,project_path,request

output=Path(output)
def project_api(project,suffix="",method="GET",body=None):
    return request(project_path(project,suffix),method,body)
def patch(project,**fields):
    return project_api(project,'/assets/groudon','PATCH',{'fields':fields})
def rename(project,old,new):
    return project_api(project,'/rename','POST',{'from':old,'to':new})
def snapshot(project):
    with sqlite3.connect(f'file:{project}/project.db?mode=ro',uri=True) as db:
        data,heads,kind=db.execute('SELECT snapshot,heads,typeof(snapshot) FROM crdt_document WHERE id=1').fetchone()
    return data,heads,kind
def clone(source,target):
    Path(target).mkdir()
    with sqlite3.connect(f'file:{source}/project.db?mode=ro',uri=True) as src, sqlite3.connect(f'{target}/project.db') as dst: src.backup(dst)
def native(mode,source,target,*extra):
    done=subprocess.run(['nice','-n','19','cargo','run','--quiet','--locked','--manifest-path',manifest,'--',mode,str(source),str(target),*extra],capture_output=True,encoding='utf-8',timeout=60)
    assert done.returncode==0,done.stderr
    return json.loads(done.stdout)
original_id=json.loads(project_api(project))['id']
before,_,_=snapshot(project)
patch(project,description='Native stream append proof.')
after,heads,kind=snapshot(project)
assert kind=='blob' and len(after)>len(before) and after.startswith(before), (kind,len(before),len(after))
assert json.loads(project_api(project))['id']==original_id, 'append changed the project ID'
(output/'crdt-stream-before.am').write_bytes(before)
(output/'crdt-stream-after.am').write_bytes(after)
# Both parser truncation and a syntactically complete older prefix must refuse. Current
# schema with no native row must not invent identity from the relational meta fallback.
for label,data in [('partial',after[:-1]),('older-prefix',before),('missing-row',None)]:
    target=output/f'crdt-bad-{label}'
    clone(project,target)
    with sqlite3.connect(target/'project.db') as db:
        kept_id=db.execute("SELECT value FROM meta WHERE key='id'").fetchone()
        if data is None: db.execute('DELETE FROM crdt_document WHERE id=1')
        else: db.execute('UPDATE crdt_document SET snapshot=? WHERE id=1',(data,))
    try:
        project_api(target)
    except Refused as refused:
        assert json.loads(refused.payload)['error']['code'] in {'project_crdt','project_db'},(label,refused.payload)
    else:
        raise AssertionError(f'{label}: read accepted corrupt native state')
    with sqlite3.connect(target/'project.db') as db:
        assert db.execute("SELECT value FROM meta WHERE key='id'").fetchone()==kept_id,label
        row=db.execute('SELECT snapshot,heads FROM crdt_document WHERE id=1').fetchone()
        assert row is None if data is None else row==(data,heads),label
# A valid merge can expose two canvas IDs with the same local key. The selected typed
# peer row must be copied into the mapped identity on the next real mapping edit.
clone(project,collision)
report=json.loads((output/'crdt-canvas-collision.json').read_text())
with sqlite3.connect(f'{collision}/project.db') as db:
    db.execute('UPDATE crdt_document SET snapshot=?,heads=? WHERE id=1',((output/'crdt-canvas-collision.am').read_bytes(),json.dumps(report['heads'])))
rename(collision,'art','surface')
(output/'crdt-canvas-after.am').write_bytes(snapshot(collision)[0])
canvas=native('--canvas-check',output/'crdt-canvas-after.am',output/'crdt-canvas-collision.json')
# Hold the real SQLite writer turn while an HTTP edit reads its old base. Only
# the exact append CAS refusal proves this. A request that reads the new base
# can succeed, in which case retry with a distinct edit.
observed=False
with ThreadPoolExecutor(max_workers=1) as worker:
    for attempt in range(8):
        source=output/f'crdt-outside-{attempt}-before.am'; source.write_bytes(snapshot(peer)[0])
        target=output/f'crdt-outside-{attempt}.am'
        report=native('--outside-edit',source,target)
        asked=f'STALE_NATIVE_{attempt}'
        lock=sqlite3.connect(f'{peer}/project.db',timeout=20)
        try:
            lock.execute('BEGIN IMMEDIATE')
            pending=worker.submit(patch,peer,description=asked)
            time.sleep(.8)
            assert not pending.done(),'HTTP edit returned while the owned write lock was held'
            seq=int(lock.execute("SELECT value FROM meta WHERE key='seq'").fetchone()[0])+1
            lock.execute('UPDATE crdt_document SET snapshot=?,heads=? WHERE id=1',(target.read_bytes(),json.dumps(report['heads'])))
            lock.execute('INSERT INTO crdt_changes(hash,seq,actor,bytes) VALUES(?,?,?,?)',(report['hash'],seq,report['actor'],target.with_suffix('.change').read_bytes()))
            lock.execute("UPDATE meta SET value=? WHERE key='seq'",(str(seq),))
            lock.commit()
        finally:
            lock.close()
        try:
            pending.result(timeout=40)
        except Refused as refused:
            error=json.loads(refused.payload)['error']
            assert refused.status==409 and error['code']=='project_changed' and 'while this edit was being committed' in error['message'],refused.payload
            held=project_api(peer)
            retained=f'retained native writer crdt-outside-{attempt}'
            assert asked not in held and retained in held,held
            patch(peer,description=asked)
            held=project_api(peer)
            assert asked in held and retained in held,held
            observed=True
            break
assert observed,'no observed stale native base; proof is inconclusive'
# Install a native peer fixture whose prefix and suffix contain independently
# selected normalized names. Seed the old projection so ordinal-only updates
# exercise real existing rows rather than comparing against a missing fixture.
mixed=output/'crdt-scan-collisions'; clone(project,mixed)
scans_before=output/'crdt-scans-before.am'
fixture=native('--scan-fixture',output/'crdt-stream-after.am',scans_before)
ord0=fixture['first_collision_order']
with sqlite3.connect(mixed/'project.db') as db:
    db.execute('UPDATE crdt_document SET snapshot=?,heads=? WHERE id=1',(scans_before.read_bytes(),json.dumps(fixture['heads'])))
    for key,name,ord,text in [('mixed name','MIXED-NAME',ord0+3,'last mixed'),('étage a','ÉTAGE_A',ord0+4,'last unicode'),('projection pivot','projection_pivot',ord0+2,'between')]:
        db.execute('INSERT INTO boxes(key,name,ord) VALUES(?,?,?)',(key,name,ord))
        db.execute('INSERT INTO fields(box,ord,field,value) VALUES(?,0,?,?)',(key,'text',json.dumps(text)))
def chosen(project):
    with sqlite3.connect(f'{project}/project.db') as db:
        return [(name,ord,json.loads(value)) for name,ord,value in db.execute("SELECT boxes.name,boxes.ord,fields.value FROM boxes JOIN fields ON fields.box=boxes.key WHERE boxes.key IN ('mixed name','étage a') AND fields.field='text' ORDER BY boxes.ord")]
project_api(mixed,'/assets/projection_pivot','DELETE')
assert chosen(mixed)==[('MIXED-NAME',ord0+2,'last mixed'),('ÉTAGE_A',ord0+3,'last unicode')],chosen(mixed)
project_api(mixed,'/assets/Mixed_Name','DELETE')
assert chosen(mixed)==[('MIXED-NAME',ord0+1,'last mixed'),('ÉTAGE_A',ord0+2,'last unicode')],chosen(mixed)
patch(mixed,e2e_keys={'keep':'kept','new':['same','same']},e2e_replace=['same','same'])
patch(mixed,e2e_replace={'branch':[1,1]})
scans_after=output/'crdt-scans-after.am';scans_after.write_bytes(snapshot(mixed)[0])
scans=native('--scan-check',scans_before,scans_after)

# A separate valid graph exercises a case-only rename through the project API. The native identity reader checks the box kept its identity.
control=output/'crdt-case';clone(project,control)
case_before=output/'crdt-case-before.am';case_before.write_bytes(snapshot(control)[0])
rename(control,'groudon','Groudon')
case_after=output/'crdt-case-after.am';case_after.write_bytes(snapshot(control)[0])
native('--identity',case_before,case_after,'groudon','Groudon')
scans.update(mixed_projection=chosen(mixed),case_only_id_retained=True)
(output/'crdt-scans-proof.json').write_text(json.dumps(scans,ensure_ascii=False)+'\n',encoding='utf-8')
print(json.dumps({'passed':True,'append_type':kind,'prefix_bytes':len(before),'after_bytes':len(after),'project_id':original_id,'partial_and_boundary_truncation_refused':True,'missing_native_row_refused':True,'outside_writer_cas_refused_then_retry':True,'canvas':canvas}))
STREAM_PROOF
# The existing suite's real fresh-runner crash/restart path, with no second home or daemon.
local prior pid
prior=$(id_of "$P")
pid=$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")
kill9
in_project "$P" --json --full status > "$OUT/crdt-restart.json" 2> "$OUT/crdt-restart.err" || check "CRDT: status did not restart Calcifer"
J --arg id "$prior" '($id | length > 0) and .project.id == $id' "$OUT/crdt-restart.json" || check "CRDT: native appended identity changed across daemon restart"
[ "$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")" != "$pid" ] || check "CRDT: Calcifer PID did not change"
db_calcifer
python3 - "$P/project.db" "$OUT/crdt-stream-after.am" <<'STREAM_RESTART' || check "CRDT: native appended stream changed on readonly restart"
import sqlite3,sys
with sqlite3.connect(f'file:{sys.argv[1]}?mode=ro',uri=True) as db:
    data,kind=db.execute('SELECT snapshot,typeof(snapshot) FROM crdt_document WHERE id=1').fetchone()
assert kind=='blob' and data==open(sys.argv[2],'rb').read()
STREAM_RESTART
}

# A project 0.24.0 made, opened by this cozy (cl-904's schema 3: its files, versions and runs leave the Automerge document for
# project.db's rows on its first open). 0.24.0's own cozy makes it, in this one's place as its release installs it; then this
# cozy goes back in its place, as an update puts it, and opens it: every take, file, word and history entry reads as 0.24.0
# read it, each take with the id it had, and Undo reaches past the upgrade.
db_migrate() {
step "project.db: a project 0.24.0 made opens here with nothing lost"
local M=$OUT/migrate was=$OUT/cozy-0.24.0 now=$OUT/cozy-now old arch c source
arch=$(uname -m); [ "$arch" != arm64 ] || arch=aarch64
mkdir -p "$was"
curl -fsSL --retry 3 --retry-all-errors "https://github.com/cozy-creator/cozy-creator/releases/download/v0.24.0/cozy-$arch-unknown-linux-gnu.tar.gz" \
  | tar -xz -C "$was" || { check "migrate: 0.24.0's archive"; return; }
old=$(find "$was" -type f -name cozy -print -quit)
cp "$COZY" "$now"
in_cozy "$old" && [ "$(cozy --version)" = "cozy 0.24.0" ] || { check "migrate: 0.24.0 in this cozy's place: $(cozy --version)"; in_cozy "$now"; return; }
echo "migrate: Calcifer $(api "http://$ADDR/v1/status" | jq -r .version) makes it"
mkdir -p "$M"; cp "$here/project/draw.py" "$M/"
yaml_in "$M" <<'YAML' || { in_cozy "$now"; return; }
version: 4
assets:
  notes: {text: Notes 0.24.0 wrote.}
  drawn:
    tool: ./draw.py/draw
    instructions: A still.
YAML
for c in red teal navy olive; do still "$M/$c.png" "$c"; done
project_import "$M" "$M/red.png" take > "$OUT/migrate-import.txt" 2>&1 || check "migrate: import take: $(tail -c 300 "$OUT/migrate-import.txt")"
for c in teal navy; do
  source="sha256:$(sha "$M/$c.png")"
  api --fail-with-body -X PUT --data-binary "@$M/$c.png" "http://$ADDR/v1/files/$source" > /dev/null || check "migrate: upload $c"
  project_api "$M" /assets/take/versions -X POST -H 'Content-Type: application/json' -d "{\"source\": \"$source\"}" > "$OUT/migrate-version.txt" 2>&1 \
    || check "migrate: take from $c: $(tail -c 300 "$OUT/migrate-version.txt")"
done
project_api "$M" /approve -X POST -H 'Content-Type: application/json' -d '{"name": "take", "version": "2"}' > /dev/null || check "migrate: approve take 2"
project_api "$M" /assets/take/versions/3 -X DELETE > /dev/null || check "migrate: discard take 3"
project_api "$M" /assets/drawn -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"instructions": "A <take.2> in teal, as 0.24.0 had it."}}' > /dev/null || check "migrate: drawn's words"
project_import "$M" "$M/olive.png" olive > "$OUT/migrate-import.txt" 2>&1 || check "migrate: import olive: $(tail -c 300 "$OUT/migrate-import.txt")"
project_api "$M" /assets/notes -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"text": "Notes 0.24.0 wrote, then changed."}}' > /dev/null || check "migrate: notes' words"
[ "$(sql1 "$M" 'PRAGMA user_version')" = 2 ] || check "migrate: 0.24.0 made schema $(sql1 "$M" 'PRAGMA user_version'), not 2"
# 0.24.0's CLI has no export, history or delete: its Calcifer's routes, as its page asks, read and write alike on both.
migrate_read "$M" 0240 || check "migrate: what 0.24.0 holds of it"
local was_0240; was_0240=$(dump_of "$M/project.db")
# A copy of it whose project.db refuses a take's row, as a migration that fails part-way would meet: below.
local F=$OUT/migrate-fails was_dump
mkdir -p "$F" && cp -a "$M/." "$F/" && rm -f "$F"/project.db*
python3 - "$M/project.db" "$F/project.db" <<'PY' || check "migrate: a copy that fails part-way"
import sqlite3, sys
with sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True) as source, sqlite3.connect(sys.argv[2]) as copy:
    source.backup(copy)
    copy.execute("CREATE TRIGGER e2e_refused BEFORE INSERT ON versions BEGIN SELECT RAISE(ABORT, 'e2e: a row refused'); END")
PY

in_cozy "$now" || { check "migrate: this cozy back in its place"; return; }
in_project "$M" --json status > "$OUT/migrate-open.json" 2> "$OUT/migrate-open.err" || { check "migrate: this cozy opening it: $(tail -c 300 "$OUT/migrate-open.err")"; return; }
[ "$(sql1 "$M" 'PRAGMA user_version')" = 3 ] || check "migrate: project.db is schema $(sql1 "$M" 'PRAGMA user_version') after its first open here, not 3"
# The project as 0.24.0 left it, kept beside it once.
local kept=$M/project.db.pre-schema3 kept_at
[ -f "$kept" ] && [ "$(dump_of "$kept")" = "$was_0240" ] || check "migrate: no copy of it as 0.24.0 left it: $([ -f "$kept" ] && dump_of "$kept") (was $was_0240)"
kept_at=$(stat -c %Y.%s "$kept" 2> /dev/null)
cozy machine logs local calcifer --lines 5000 2> /dev/null | grep -q "project.db before schema 3 kept at .*/migrate/project.db.pre-schema3" \
  || check "migrate: Calcifer's log doesn't say where the copy is"
migrate_read "$M" now || check "migrate: what this cozy holds of it"
local part
for part in facts rows yaml history; do
  cmp -s "$OUT/migrate-0240-$part" "$OUT/migrate-now-$part" || check "migrate: its $part read otherwise here: $(diff "$OUT/migrate-0240-$part" "$OUT/migrate-now-$part" | head -c 800)"
done
[ "$(sql1 "$M" "SELECT count(*) FROM versions WHERE coalesce(json_extract(record, '\$.id'), '') = ''")" = 0 ] || check "migrate: a take has no id"
for f in $(status "$M" | jq -r '.assets[].name' | while read -r name; do held "$M" "$name" | jq -r '.versions[].files[].path // empty'; done); do
  [ -f "$M/$f" ] || check "migrate: $f is gone"
done
# Undo reaches what 0.24.0 did: notes' words back, then changed again. (A box 0.24.0 deleted isn't one: it says no time, so cl-919
# lets its files go at once.)
local words='.asset.fields.text'
project_api "$M" /history/undo -X POST -H 'Content-Type: application/json' -d '{}' > "$OUT/migrate-undo.txt" 2>&1 || check "migrate: undo notes' words: $(tail -c 300 "$OUT/migrate-undo.txt")"
[ "$(held "$M" notes | jq -r "$words")" = "Notes 0.24.0 wrote." ] || check "migrate: notes undone say '$(held "$M" notes | jq -r "$words")'"
project_api "$M" /history/redo -X POST -H 'Content-Type: application/json' -d '{}' > "$OUT/migrate-undo.txt" 2>&1 || check "migrate: redo notes' words: $(tail -c 300 "$OUT/migrate-undo.txt")"
[ "$(held "$M" notes | jq -r "$words")" = "Notes 0.24.0 wrote, then changed." ] || check "migrate: notes redone say '$(held "$M" notes | jq -r "$words")'"
# A take after it: number 4 (3 is discarded), its own id, all four there across Calcifer killed.
source="sha256:$(sha "$M/olive.png")"
project_api "$M" /assets/take/versions -X POST -H 'Content-Type: application/json' -d "{\"source\": \"$source\"}" > "$OUT/migrate-version.txt" 2>&1 \
  || check "migrate: a take after: $(tail -c 300 "$OUT/migrate-version.txt")"
kill9
J '[.versions[].number] == [1, 2, 4] and .discarded == [3] and .asset.approved == 2' <<< "$(held "$M" take)" \
  || check "migrate: take after a new take and a kill: $(held "$M" take | jq -c '{versions: [.versions[].number], discarded, approved: .asset.approved}')"
[ "$(sql "$M" "SELECT json_extract(record, '\$.id') FROM versions WHERE key = 'take'" | sort -u | wc -l)" = 4 ] || check "migrate: take's four versions have not four ids"
# A migration that fails part-way, past its rows' deletes, leaves the project as it was: one transaction. Let be, it migrates.
was_dump=$(dump_of "$F/project.db")
if in_project "$F" --json status > "$OUT/migrate-fails.json" 2>&1; then check "migrate: a migration refused part-way opened the project"; fi
[ "$(dump_of "$F/project.db")" = "$was_dump" ] || check "migrate: a migration refused part-way changed project.db ($was_dump, now $(dump_of "$F/project.db")): $(tail -c 300 "$OUT/migrate-fails.json")"
db_sql "$F" "DROP TRIGGER e2e_refused"
in_project "$F" --json status > "$OUT/migrate-fails.json" 2>&1 || check "migrate: let be, the copy didn't open: $(tail -c 300 "$OUT/migrate-fails.json")"
[ "$(sql1 "$F" 'PRAGMA user_version')" = 3 ] || check "migrate: let be, the copy is schema $(sql1 "$F" 'PRAGMA user_version')"
migrate_facts "$F" | cmp -s - "$OUT/migrate-0240-facts" || check "migrate: let be, the copy reads otherwise: $(migrate_facts "$F" | diff "$OUT/migrate-0240-facts" - | head -c 600)"
[ "$(stat -c %Y.%s "$kept" 2> /dev/null)" = "$kept_at" ] || check "migrate: the copy kept beside it was written again"
[ "$(dump_of "$F/project.db.pre-schema3")" = "$was_dump" ] || check "migrate: the refused copy's own kept copy isn't it as it was"
echo "migrate: 0.24.0's project read here as it read there: $(jq -sc 'map({(.name): [.versions[].number]}) | add' "$OUT/migrate-now-facts")"
}
# in_cozy <file>: that cozy in this one's place, as an update puts it, and its Calcifer the one serving: the last stopped as a
# service manager stops one, and started by the next verb.
in_cozy() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid
  pid=$(jq -r .pid < "$lock" 2> /dev/null) || pid=''
  [ -z "$pid" ] || { kill -TERM "$pid" 2> /dev/null; while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done; }
  cp "$1" "$COZY.new" && chmod +x "$COZY.new" && mv -f "$COZY.new" "$COZY" && cozy --json machine ls > /dev/null && db_calcifer
}
# migrate_facts <project>: each box's words, takes (number, approved, files by hash) and discarded, as the API reads them.
migrate_facts() {
  local name
  status "$1" | jq -r '.assets[].name' | sort | while read -r name; do
    held "$1" "$name" | jq -c --arg n "$name" '{name: $n, fields: .asset.fields, approved: .asset.approved, versions: [.versions[] | {number, files: (.files | map_values(.sha256))}], discarded: (.discarded // [])}'
  done
}
# migrate_read <project> <tag>: what it holds, to $OUT/migrate-<tag>-{facts,rows,yaml,history}.
migrate_read() {
  migrate_facts "$1" > "$OUT/migrate-$2-facts" && migrate_rows "$1" > "$OUT/migrate-$2-rows" \
    && project_api "$1" /yaml | yaml_body > "$OUT/migrate-$2-yaml" \
    && project_api "$1" /history | jq -c '[.entries[] | {words, undone: (.undone != null)}]' > "$OUT/migrate-$2-history"
}
# dump_of <project.db>: its schema version and the hash of all it holds (read; a WAL copy no one has open needs its -shm made).
dump_of() {
  python3 - "$1" <<'PY'
import hashlib, sqlite3, sys
conn = sqlite3.connect(sys.argv[1])
held = hashlib.sha256("\n".join(conn.iterdump()).encode()).hexdigest()
print(conn.execute("PRAGMA user_version").fetchone()[0], held)
PY
}
# migrate_rows <project>: project.db's rows of takes, boxes' records and files, sorted.
migrate_rows() {
  local q
  for q in "SELECT key, number, discarded, record FROM versions" "SELECT key, record FROM lock_assets" "SELECT sha256, record FROM lock_files"; do sql "$1" "$q" | sort; done
}
