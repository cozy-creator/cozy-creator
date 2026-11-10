# e2e/cli/projectdb.sh: a project's state is one SQLite database, project.db (cl-803), each cell through the CLI and Calcifer's HTTP
# as the page calls it, no machine or model needed.
#   db_first_open   a folder of the files from before (format 4, history, canvas, versions) imports once, its files deleted, undo goes on
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
db_existing_projects
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
    undo | offline) STOP=$(web_halt "$lock") START=$(web_start) web_proof "$name" "$chrome" "$pw" "$restart" ;;
    held) web_held "$chrome" "$pw" ;;
    *) web_proof "$name" "$chrome" "$pw" '' ;;
  esac
done
}

projectdb() {
db_first_open
db_concurrent
db_crash
db_restart
db_roundtrip
db_latency
}

db_first_open() {
step "project.db: a folder of the files from before imports once on first open, its files deleted, its history going on"
local F=$OUT/db-legacy a b answer
mkdir -p "$F/files"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=64x64" -frames:v 1 "$OUT/db-a.png"
ffmpeg -v error -y -f lavfi -i "color=c=olive:size=64x64" -frames:v 1 "$OUT/db-b.png"
a=$(sha "$OUT/db-a.png") b=$(sha "$OUT/db-b.png")
cp "$OUT/db-a.png" "$F/files/$a.png"; cp "$OUT/db-b.png" "$F/files/$b.png"
cat > "$F/project.yaml" <<YAML
version: 4
# A hand's comment: the database holds values, so it isn't kept.
assets:
  art:
    approved: ${a:0:12}
    description: The art.
  shot:
    description: <art> at dusk, its takes.
    aspect_ratio: "16:9"
  mood:
    text: Quiet.
YAML
cat > "$F/project.lock" <<YAML
format: 3
files: {}
assets:
  shot:
    name: shot
    kind: image
    numbered: 3
    versions:
      - {number: 1, inputs: "0000000000000001", files: {image: "sha256:$b"}, seed: 7}
      - {number: 2, inputs: "0000000000000002", files: {image: "sha256:$a"}, seed: 8}
    discarded:
      - {number: 3, inputs: "0000000000000003", files: {image: "sha256:$b"}}
YAML
printf '# Where cards sit. Only the page reads this. Nothing here is an input.\nversion: 1\ncards:\n  art: {x: 10, y: 20}\n  shot: {x: 400, y: 20, w: 360}\n' > "$F/project.canvas.yaml"
cat > "$F/.history.json" <<'JSON'
{"next": 2, "entries": [
  {"id": 1, "at_ms": 1794000000000, "by": {"via": "cli"}, "words": "Deleted sky", "changes": [{"kind": "asset", "name": "sky", "before": "assets:\n  sky:\n    text: A blue sky.\n", "after": null}]},
  {"id": 2, "at_ms": 1794000001000, "by": {"via": "page", "client": "tab1"}, "words": "Set aspect_ratio on shot", "changes": [{"kind": "fields", "name": "shot", "fields": {"aspect_ratio": ["1:1", "16:9"]}}]}]}
JSON
printf '{}' > "$F/.measured.json"; printf '{}' > "$F/.keys.json"
in_project "$F" status > "$OUT/db-legacy.txt" 2>&1 || check "first open: status: $(tail -c 300 "$OUT/db-legacy.txt")"
local left; left=$(cd "$F" && ls -A | grep -vxE 'files|project\.db(-wal|-shm)?' | tr '\n' ' ' || true)
echo "first open: the folder now holds $(cd "$F" && ls -A | tr '\n' ' ')"
[ -f "$F/project.db" ] && [ -z "$left" ] || check "first open: project.db made and the files gone, but it holds: ${left:-no project.db}"
J '[.versions[].number] == [1, 2] and .discarded == [3]' <<< "$(held "$F" shot)" || check "first open: shot's takes: $(held "$F" shot | jq -c '{v: [.versions[].number], d: .discarded}')"
[ "$(sql "$F" "SELECT name, x, y, w FROM canvas ORDER BY name" | tr -d '\n')" = '["art", 10.0, 20.0, null]["shot", 400.0, 20.0, 360.0]' ] || check "first open: the rows: $(sql "$F" 'SELECT name, x, y, w FROM canvas')"
in_project "$F" --json history > "$OUT/db-legacy-history.json" || check "first open: history"
J '[.entries[].words] == ["Set aspect_ratio on shot", "Deleted sky"]' "$OUT/db-legacy-history.json" || check "first open: its history: $(jq -c '[.entries[].words]' "$OUT/db-legacy-history.json")"
# Undo goes on across the cut: entry 1 put back from the lines .history.json kept.
in_project "$F" undo 1 > "$OUT/db-legacy-undo.txt" 2>&1 || check "first open: undo 1: $(tail -c 300 "$OUT/db-legacy-undo.txt")"
J '.assets | any(.name == "sky")' <<< "$(status "$F")" || check "first open: sky not back after undo 1"
answer=$(in_project "$F" export)
grep -q 'text: A blue sky.' <<< "$answer" || grep -q 'text: "A blue sky."' <<< "$answer" || check "first open: sky's words: $answer"
grep -q 'comment' <<< "$answer" && check "first open: a comment came through: $answer"
in_project "$F" undo 2 > /dev/null 2>&1 || check "first open: undo 2"
in_project "$F" export | grep -q 'aspect_ratio: "1:1"' || check "first open: undo 2 didn't put 1:1 back"
# Opened again, nothing more happens.
local before; before=$(sql1 "$F" "SELECT value FROM meta WHERE key = 'seq'")
in_project "$F" status > /dev/null 2>&1 || check "first open: second open"
[ "$(sql1 "$F" "SELECT value FROM meta WHERE key = 'seq'")" = "$before" ] || check "first open: a second open wrote"
echo "first open: $(held "$F" shot | jq -c '{takes: [.versions[].number], discarded}'), history $(jq -c '[.entries[].id]' "$OUT/db-legacy-history.json"), undo across the cut ok"
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
mkdir -p "$K"
yaml="version: 4\nassets:\n"
for ((i = 1; i <= n; i++)); do yaml+="  m_$i: {text: Marker $i.}\n  r_$i: {text: Renamed $i.}\n"; done
printf "$yaml" > "$K/project.yaml"
in_project "$K" status > /dev/null 2>&1 || { check "crash: open"; return; }
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
(cd "$L" && in_project "$L" import "$L/clips/0.mp4" take > /dev/null) || { check "latency: import the first take"; return; }
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
