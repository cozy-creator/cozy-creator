# e2e/cli/projectdb.sh: a project's state is one SQLite database, project.db (cl-803), each cell through the CLI and Calcifer's HTTP
# as the page calls it, no machine or model needed.
#   db_first_open   current creation/reopen and stable undo identity; old physical project formats are refused
#   db_concurrent   the CLI and the page write at once, one box's fields, new boxes and rows: no write is lost
#   db_crash        Calcifer killed (kill -9) amid writes of a box, its row and its push-aside: each whole or absent; a resend is done once
#   db_restart      undo and redo across Calcifer killed
#   db_roundtrip    export, import of it changes nothing; an edited export imports as one entry, a renamed box keeps its takes
#   db_latency      write latency at a project with 500 takes (it runs on the cozy before project.db too: --shard projectdb-latency)
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
# The page's own writes: its client, a key per action.
page() { local method=$1 path=$2 body=$3; api -X "$method" -H 'Content-Type: application/json' -H 'Cozy-Client: page e2e-tab' -H "Idempotency-Key: page-$RANDOM$RANDOM$RANDOM" -d "$body" "http://$ADDR/v1/projects/$ID$path"; }
id_of() { status "$1" | jq -r .id; }
# kill9 Calcifer by its lock's pid, and wait for it gone.
kill9() {
  local pid; pid=$(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")
  kill -9 "$pid" 2> /dev/null || true
  while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done
}
tiny() { base64 -d > "$1" <<< 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=='; }

# The cells from before project.db that it changes most: the project setup and its first eleven, and the page's
# tools, versions, nodes, place, undo, live, layout, offline and held.
db_existing() {
tools_read
db_first_open
db_crash
db_existing_web
}

db_existing_projects() {
step "a project: new, named, a folder of it, -p"
local P=$OUT/garden ID
starter_project "$P" > "$OUT/project-new.txt" 2>&1 || check "project new failed: $(tail -c 300 "$OUT/project-new.txt")"
[ "$(ls -A "$P" 2>/dev/null | grep -vxE 'project\.db-(wal|shm)')" = project.db ] || check "project new made $(ls -A "$P" 2>/dev/null | tr '\n' ' '), not only project.db"
mkdir -p "$P/sub"
# A subdirectory uses the project holding it.
(cd "$P/sub" && cozy --json status) > "$OUT/project-cwd.json" || check "status did not use its working directory"
J --arg p "$P" '.path == $p' "$OUT/project-cwd.json" || check "status did not find the containing project"
in_project "$P/sub" --json status > "$OUT/project-status.json" 2> "$OUT/project-status.err" || check "status of a folder in it failed: $(tail -c 300 "$OUT/project-status.err")"
local cell
# Each on its own: one that stops on its `set -e` is said, and the next goes on.
for cell in cut_of_takes said_once film_is_its_file short_film_heals cut_versions by_asset hard_cut upgrades first_open version_text old_landing; do ( $cell ) || check "$cell stopped part way (exit $?)"; done
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
    *) web_proof "$name" "$chrome" "$pw" '' ;;
  esac
  [ "$name" != scale ] || web_scale_said
done
}

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

# The deleted asset CLI's concurrent-write, undo/redo and YAML roundtrip cases
# are retired; the retained checks exercise the project API and storage directly.
projectdb() {
db_crdt
db_cancel_undo
db_first_open
db_crash
db_latency
}

db_first_open() {
step "project.db: current creation/reopen and stable undo identities; old physical formats are refused"
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
local old file digest
for file in project.yaml project.lock project.db.new; do
  old=$OUT/refused-${file//./-}
  mkdir -p "$old"
  printf 'version: 4\nassets: {}\n' > "$old/$file"
  digest=$(sha "$old/$file")
  if (cd "$old" && cozy --json status) > "$old/refusal.json" 2> "$old/refusal.err"; then check "old format $file was accepted"; fi
  grep -q project_format_unsupported "$old/refusal.json" "$old/refusal.err" || check "old format $file did not explain current-format requirement"
  [ "$(sha "$old/$file")" = "$digest" ] && [ ! -e "$old/project.db" ] && [ ! -e "$old/.cozy-legacy-v1" ] && [ ! -e "$old/project.db.v1" ] || check "old format $file was changed/imported"
done
}

db_concurrent() {
step "project.db: the CLI and the page write at once: no write is lost"
local P=$OUT/db-together n=30 i
starter_project "$P" > /dev/null 2>&1 || { check "together: project new"; return; }
db_calcifer; ID=$(id_of "$P")
local seq; seq=$(api "http://$ADDR/v1/projects/$ID/canvas" | jq .seq)
# The CLI: a field of kyogre each time, and a box of its own. The page: another field of kyogre, its own boxes, and rows it moves.
(for ((i = 1; i <= n; i++)); do
  in_project "$P" set kyogre "cli_$i=$i" > /dev/null 2>> "$OUT/db-together-cli.err" || echo "set cli_$i" >> "$OUT/db-together-lost"
  in_project "$P" add text "cli_box_$i" "text=From the CLI $i." > /dev/null 2>> "$OUT/db-together-cli.err" || echo "add cli_box_$i" >> "$OUT/db-together-lost"
done) &
local cli=$!
(for ((i = 1; i <= n; i++)); do
  page PATCH /assets/kyogre "{\"fields\": {\"page_$i\": $i}}" > /dev/null || echo "patch page_$i" >> "$OUT/db-together-lost"
  page POST /assets "{\"tool\": \"\", \"name\": \"page_box_$i\", \"exact\": true, \"fields\": {\"text\": \"From the page $i.\"}, \"at\": {\"x\": $((i * 400)), \"y\": 900}}" > /dev/null || echo "create page_box_$i" >> "$OUT/db-together-lost"
  page PATCH /canvas "{\"cards\": {\"groudon\": {\"x\": $i, \"y\": $((i * 2))}}}" > /dev/null || echo "move $i" >> "$OUT/db-together-lost"
done) &
local pg=$!
wait "$cli" "$pg"
[ ! -s "$OUT/db-together-lost" ] || check "together: requests failed: $(head -5 "$OUT/db-together-lost") $(tail -c 300 "$OUT/db-together-cli.err")"
in_project "$P" export > "$OUT/db-together.yaml"
local missing=()
for ((i = 1; i <= n; i++)); do
  grep -q "^    cli_$i: $i$" "$OUT/db-together.yaml" || missing+=("cli_$i")
  grep -q "^    page_$i: $i$" "$OUT/db-together.yaml" || missing+=("page_$i")
  grep -q "^  cli_box_$i:" "$OUT/db-together.yaml" || missing+=("cli_box_$i")
  grep -q "^  page_box_$i:" "$OUT/db-together.yaml" || missing+=("page_box_$i")
done
[ ${#missing[@]} = 0 ] || check "together: lost writes: ${missing[*]}"
! grep -qE '^  (cli|page)_box_[0-9]+_[0-9]+:' "$OUT/db-together.yaml" || check "together: a box was made twice: $(grep -E '^  (cli|page)_box_[0-9]+_[0-9]+:' "$OUT/db-together.yaml" | head -3)"
[ "$(sql "$P" "SELECT x, y, by FROM canvas WHERE name = 'groudon'" | tr -d '\n')" = "[$n.0, $((n * 2)).0, \"page e2e-tab\"]" ] || check "together: groudon's row: $(sql "$P" "SELECT x, y, by FROM canvas WHERE name = 'groudon'")"
local entries; entries=$(in_project "$P" --json history -n 200 | jq '.entries | length')
echo "together: all $((5 * n)) writes kept (fields, boxes and rows, from both), $entries history entries, groudon at its last row"
[ "$entries" -ge $((5 * n)) ] || check "together: $entries history entries for $((5 * n)) edits"
}

db_crash() {
step "project.db: Calcifer killed amid writes: each is whole or absent, and its resend is done once"
local K=$OUT/db-crash n=150 i seq yaml writer answer
run_project "$K" > "$OUT/db-crash-new.txt" 2>&1 || { check "crash: native bootstrap"; return; }
yaml="version: 4\nassets:\n"
for ((i = 1; i <= n; i++)); do yaml+="  m_$i: {text: Marker $i.}\n  r_$i: {text: Renamed $i.}\n"; done
printf '%b' "$yaml" > "$OUT/db-crash-seed.yaml"
project_api "$K" /yaml -X PUT -H 'Content-Type: application/yaml' --data-binary "@$OUT/db-crash-seed.yaml" > "$OUT/db-crash-seed.txt" 2>&1 || { check "crash: seed current native project"; return; }
db_calcifer; ID=$(id_of "$K")
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

db_restart() {
step "project.db: undo and redo across Calcifer killed"
local U=$OUT/db-undo
starter_project "$U" > /dev/null 2>&1 || { check "undo: project new"; return; }
in_project "$U" export > "$OUT/db-undo-0.yaml"
in_project "$U" set kyogre aspect_ratio=1:1 > /dev/null || check "undo: set"
in_project "$U" delete coast > /dev/null || check "undo: delete"
in_project "$U" set groudon at=700,700 > /dev/null || check "undo: move"
in_project "$U" export > "$OUT/db-undo-3.yaml"
kill9
local k
for k in 1 2 3; do in_project "$U" undo > "$OUT/db-undo-step.txt" 2>&1 || check "undo: undo $k after the kill: $(tail -c 300 "$OUT/db-undo-step.txt")"; done
in_project "$U" export | cmp -s - "$OUT/db-undo-0.yaml" || check "undo: three undone isn't what it was: $(in_project "$U" export | diff - "$OUT/db-undo-0.yaml" | head -10)"
kill9
for k in 1 2 3; do in_project "$U" redo > "$OUT/db-undo-step.txt" 2>&1 || check "undo: redo $k after the kill: $(tail -c 300 "$OUT/db-undo-step.txt")"; done
in_project "$U" export | cmp -s - "$OUT/db-undo-3.yaml" || check "undo: three redone isn't what it was: $(in_project "$U" export | diff - "$OUT/db-undo-3.yaml" | head -10)"
db_calcifer
[ "$(sql "$U" "SELECT x, y FROM canvas WHERE name = 'groudon'" | tr -d '\n')" = '[700.0, 700.0]' ] || check "undo: groudon's row after redo: $(sql "$U" "SELECT x, y FROM canvas WHERE name = 'groudon'")"
echo "undo: three undone and redone, each across a kill -9: $(in_project "$U" --json history | jq -c '[.entries[] | {id, undone: (.undone != null)}]')"
}

db_roundtrip() {
step "project.db: export then import changes nothing; an edited export is one entry, a renamed box keeps its takes"
local R=$OUT/db-roundtrip pic=$OUT/db-roundtrip.png next=$OUT/db-roundtrip-2.png before after
starter_project "$R" > /dev/null 2>&1 || { check "roundtrip: project new"; return; }
tiny "$pic"; ffmpeg -v error -y -f lavfi -i "color=c=navy:size=8x8" -frames:v 1 "$next"
(cd "$R" && in_project "$R" import "$pic" art > /dev/null) || check "roundtrip: import art"
(cd "$R" && in_project "$R" import "$next" --into art > /dev/null) || check "roundtrip: art's next version"
in_project "$R" export -o "$OUT/db-roundtrip-1.yaml" > /dev/null || check "roundtrip: export"
before=$(in_project "$R" --json history | jq '.entries | length')
(cd "$R" && in_project "$R" import "$OUT/db-roundtrip-1.yaml" > /dev/null) || check "roundtrip: import of the export"
in_project "$R" export | cmp -s - "$OUT/db-roundtrip-1.yaml" || check "roundtrip: export, import, export differ"
[ "$(in_project "$R" --json history | jq '.entries | length')" = "$before" ] || check "roundtrip: importing what it holds made an entry"
# A hand's edit of the export: a field, a rename, a box gone and one added.
perl -pe 's/aspect_ratio: "16:9"/aspect_ratio: "4:3"/ if $. < 12; s/^  art:/  painting:/; s/<art>/<painting>/g' "$OUT/db-roundtrip-1.yaml" | perl -0pe 's/\n  coast:\n(    .*\n)+//' > "$OUT/db-roundtrip-2.yaml"
printf '\n  added:\n    text: New words.\n' >> "$OUT/db-roundtrip-2.yaml"
(cd "$R" && in_project "$R" import "$OUT/db-roundtrip-2.yaml" > "$OUT/db-roundtrip-import.txt" 2>&1) || check "roundtrip: import of the edit: $(tail -c 300 "$OUT/db-roundtrip-import.txt")"
after=$(status "$R" || true)
J '.assets | map(.name) | (index("painting") != null) and (index("art") == null) and (index("coast") == null) and (index("added") != null)' <<< "$after" || check "roundtrip: after the edit: $(jq -c '[.assets[].name]' <<< "$after")"
J '.versions | length == 2' <<< "$(held "$R" painting)" || check "roundtrip: the renamed box's versions: $(held "$R" painting | jq -c '[.versions[]?.number]')"
[ "$(in_project "$R" --json history | jq '.entries | length')" = $((before + 1)) ] || check "roundtrip: the import is not one entry: $(in_project "$R" --json history | jq -c '[.entries[].words]')"
in_project "$R" undo > /dev/null || check "roundtrip: undo the import"
in_project "$R" export > "$OUT/db-roundtrip-3.yaml"
# Undone, every box is back, the renamed one under its old name; one put back goes at the end.
[ "$(grep -c '^  [a-z_]*:' "$OUT/db-roundtrip-3.yaml")" = "$(grep -c '^  [a-z_]*:' "$OUT/db-roundtrip-1.yaml")" ] && grep -q '^  art:' "$OUT/db-roundtrip-3.yaml" && grep -q '^  coast:' "$OUT/db-roundtrip-3.yaml" && ! grep -q '^  added:' "$OUT/db-roundtrip-3.yaml" \
  || check "roundtrip: undone isn't what it was: $(diff "$OUT/db-roundtrip-1.yaml" "$OUT/db-roundtrip-3.yaml" | head -10)"
echo "roundtrip: export and import stable, the edit one entry, painting kept its 2 versions, undone"
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
[ "$(sql1 "$P" 'PRAGMA user_version')" = 2 ] || { check "CRDT: project.db is not schema 2"; return; }
[ "$(sql1 "$P" 'SELECT count(*) FROM crdt_document WHERE format = 1 AND length(snapshot) > 0')" = 1 ] || check "CRDT: no native snapshot"
project_api "$P" /yaml -X PUT -H 'Content-Type: application/yaml' --data-binary @- <<'YAML' || { check "CRDT: seed the editable graph"; return; }
version: 4
assets:
  kyogre:
    tool: qwen-image-2/image
    instructions: A whale.
  groudon:
    tool: qwen-image-2/image
    instructions: On shore with <kyogre>.
YAML
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
project_api "$P" /yaml > "$OUT/crdt-export.yaml" || check "CRDT: export"
before=$(sql1 "$P" 'SELECT heads FROM crdt_document WHERE id=1')
project_api "$P" /yaml -X PUT -H 'Content-Type: application/yaml' --data-binary "@$OUT/crdt-export.yaml" > "$OUT/crdt-roundtrip.txt" 2>&1 || check "CRDT: unchanged import"
after=$(sql1 "$P" 'SELECT heads FROM crdt_document WHERE id=1')
[ "$before" = "$after" ] || check "CRDT: unchanged YAML export/import rewrote causal history"
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
project_api "$P" /yaml > "$OUT/crdt-cache-export.yaml" || check "CRDT: read after projection damage"
project_api "$P" /history > "$OUT/crdt-cache-history.json" || check "CRDT: history after projection damage"
{ ! grep -q QUERY_CACHE_ONLY "$OUT/crdt-cache-export.yaml" && ! grep -q QUERY_CACHE_ONLY "$OUT/crdt-cache-history.json"; } || check "CRDT: a relational projection overrode canonical state/history"
# An exported edit carries a graph-body base. A subsequent API edit must not be overwritten
# by an old file; runtime/canvas-only changes do not participate in that precondition.
project_api "$P" /yaml > "$OUT/crdt-stale-export.yaml" || check "CRDT: export edit base"
project_api "$P" /assets/groudon -X PATCH -H 'Content-Type: application/json' -d '{"fields":{"description":"An intervening API edit."}}' > "$OUT/crdt-intervening-set.txt" 2>&1 || check "CRDT: intervening API edit"
if project_api "$P" /yaml -X PUT -H 'Content-Type: application/yaml' --data-binary "@$OUT/crdt-stale-export.yaml" > "$OUT/crdt-stale-import.json" 2> "$OUT/crdt-stale-import.err"; then
  check "CRDT: stale same-project export overwrote a newer API edit"
fi
grep -q project_changed "$OUT/crdt-stale-import.json" "$OUT/crdt-stale-import.err" || check "CRDT: stale import did not explain project_changed"
project_api "$P" /yaml > "$OUT/crdt-stale-after.yaml" || check "CRDT: export after stale refusal"
grep -q 'An intervening API edit.' "$OUT/crdt-stale-after.yaml" || check "CRDT: stale import erased the intervening edit"
# Cargo consumes the same published native library as the product. This is a format/merge proof,
# not another daemon or a substitute project storage implementation.
CARGO_BUILD_JOBS=1 nice -n 19 cargo run --quiet --locked --manifest-path "$here/crdt/Cargo.toml" -- "$OUT/crdt-before.am" "$OUT/crdt-renamed.am" > "$OUT/crdt-native.json" 2> "$OUT/crdt-native.err" \
  || check "CRDT: native format/replica proof: $(tail -c 400 "$OUT/crdt-native.err")"
J '.passed and .native_replicas == 3 and .original_versions == 2 and .same_number_versions_preserved == 2 and (.prompt_alternatives == ["dawn","dusk","noon"])' "$OUT/crdt-native.json" \
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
            exported=project_api(peer,'/yaml')
            retained=f'retained native writer crdt-outside-{attempt}'
            assert asked not in exported and retained in exported,exported
            patch(peer,description=asked)
            exported=project_api(peer,'/yaml')
            assert asked in exported and retained in exported,exported
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

# A separate valid graph exercises case-only rename and an arbitrary reorder
# through the project API. The native identity reader checks both retained boxes.
control=output/'crdt-case-order';clone(project,control)
case_before=output/'crdt-case-before.am';case_before.write_bytes(snapshot(control)[0])
rename(control,'groudon','Groudon')
case_after=output/'crdt-case-after.am';case_after.write_bytes(snapshot(control)[0])
native('--identity',case_before,case_after,'groudon','Groudon')
with sqlite3.connect(control/'project.db') as db:
    ordered=[row[0] for row in db.execute("SELECT name FROM boxes WHERE key IN ('groudon','whale') ORDER BY ord")]
assert set(ordered)=={'Groudon','whale'},ordered
wanted=list(reversed(ordered))
reorder=output/'crdt-reordered.yaml'
reorder.write_text('version: 4\nassets:\n'+''.join(f'  {name}: {{text: Reordered {name}}}\n' for name in wanted),encoding='utf-8')
project_api(control,'/yaml','PUT',reorder.read_bytes())
with sqlite3.connect(control/'project.db') as db:
    projected=db.execute('SELECT name,ord FROM boxes ORDER BY ord').fetchall()
assert projected==list(zip(wanted,range(2))),projected
reordered=output/'crdt-reordered.am';reordered.write_bytes(snapshot(control)[0])
for name in wanted:
    native('--identity',case_after,reordered,name)
scans.update(mixed_projection=chosen(mixed),case_only_id_retained=True,reordered_ids_retained=True)
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
