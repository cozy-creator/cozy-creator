# e2e/cli/projectdb.sh: a project's state is one SQLite database, project.db (cl-803), each cell through the CLI and Calcifer's HTTP
# as the page calls it, no machine or model needed.
#   db_first_open   current creation/reopen and stable undo identity; old physical project formats are refused
#   db_crash        Calcifer killed (kill -9) amid writes of a box, its row and its push-aside: each whole or absent; a resend is done once
#   db_crdt         the native CRDT: stable identities, old-schema refusal, projection damage, replica merges, append and restart
#   db_latency      write latency at a project with 500 takes (it runs on the cozy before project.db too: --shard projectdb-latency)
#   db_migrate      a project 0.24.0 made opens here with nothing lost: its takes, files and words, ids and history
#   db_migrate_large  a 5,000-box project of schema 3 opens here as $SCHEMA with nothing lost, and its rows read
#   db_waits        project.db held by another program while a `cozy run` begins: waited out within Calcifer's wait and past it
#   db_deleted_gc   a deleted box resolves nowhere; Undo in the window; its files and thumbnails gone after it, across a stopped Calcifer too
#   db_purged       Undo past the 3-hour purge brings each box back where it was without its files, saying so
#   db_closes       a project closes once no call, stream or render reads it; Calcifer's heap comes back; a reopen is as quick
# Uses project.sh's `api`, `status`, `held` and `sha`.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# project.db's schema (db.rs VERSION): what a project this cozy opens holds, and what its copy before a migration is named after.
SCHEMA=5

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

# The cells project.db changes most: its first open and a crash.
db_existing() {
tools_read
db_first_open
db_crash
}

# undo_window <lock> <seconds, or empty> [command]: Calcifer started again with config.toml's calcifer.undo_seconds at that, else none:
# the old one gone first, so the new one reads it. The command runs while it's stopped.
undo_window() {
  local config=$HOME/.cozy/config.toml
  awk -v n="$2" '/^undo_seconds *=/ { next } { print } /^\[calcifer\]/ && n != "" { print "undo_seconds = " n; put = 1 } END { if (n != "" && !put) printf "\n[calcifer]\nundo_seconds = %s\n", n }' "$config" > "$config.next" && mv "$config.next" "$config"
  calcifer_halt || true
  [ -z "${3:-}" ] || "$3"
  cozy --json machine ls > /dev/null 2>&1 || true
}

# calcifer_halt: Calcifer stopped as a service manager stops it (SIGTERM, a clean exit, which systemd's on-failure doesn't restart), and
# gone. One still running 120 s on is a failure, not a hang.
calcifer_halt() {
  local pid waited=0
  pid=$(jq -r .pid < "$HOME/.cozy/calcifer/calcifer.lock.holder")
  kill -TERM "$pid" 2> /dev/null || true
  while kill -0 "$pid" 2> /dev/null; do
    [ "$waited" -lt 600 ] || { check "Calcifer $pid still runs 120 s after SIGTERM"; kill -9 "$pid" 2> /dev/null || true; return 1; }
    sleep 0.2; waited=$((waited + 1))
  done
}
# in_files <project> <sha256:hex or hex>: a file of that hash is in the project's files/.
in_files() { local hex=${2#sha256:}; find "$1/files" -name "$hex*" 2> /dev/null | grep -q .; }
# derived <project> <sha256:hex or hex>: what Calcifer made of that file and keeps by its hash (thumbnails, posters, clips), wherever.
derived() { local hex=${2#sha256:}; find "$HOME/.cozy/cache" "$1/.vault-calcifer" \( -name "$hex-*" -o -name "$hex.*" \) 2> /dev/null; }
# entry_of <project> <words>: the newest history entry with those words, not undone.
entry_of() { project_api "$1" '/history?limit=50' | jq -r --arg w "$2" '[.entries[] | select(.words == $w and (.undone | not))][0].id // empty'; }
# place_of <project> <box>: where its card is, `x,y`.
place_of() { sql "$1" "SELECT x, y FROM canvas WHERE name = '$2'" | jq -r 'map(round | tostring) | join(",")'; }
# asset_of <project> <box>: the box as `GET …/assets/<box>` answers it (`.asset`), else nothing.
asset_of() { { held "$1" "$2" 2> /dev/null || true; } | jq -c '.asset // empty' 2> /dev/null || true; }
# box_kept <project> <box>: it's there again, with no file and no take.
box_kept() { asset_of "$1" "$2" | J '(.sha256 // null) == null and ((.versions // []) | length) == 0'; }

# cl-919 (Paul, 11-12: "When I delete a node, the image should be gone from the project"), with no page: a deleted box resolves nowhere,
# Undo brings it back within the window (config.toml's calcifer.undo_seconds, cut to 40 s), and once the window ends its file and what
# Calcifer made of it leave the project, unless another box holds the same file; Undo then brings the box back without them, saying so.
# One deleted while Calcifer is stopped across its window is gone once Calcifer starts. The page's own half is the delete-undo cell.
db_deleted_gc() {
step "deleted boxes: resolve nowhere, Undo in the window, their files gone after it, across a stopped Calcifer too (cl-919)"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder P=$OUT/deleted-gc window=40 pic twin late entry began code=0 said='' before after c
undo_window "$lock" "$window"
run_project "$P" > "$OUT/deleted-gc-new.log" 2>&1 || { check "deleted gc: no project: $(tail -c 300 "$OUT/deleted-gc-new.log")"; undo_window "$lock" ''; return; }
for c in red blue green; do ffmpeg -v error -y -f lavfi -i "color=c=$c:size=64x48" -frames:v 1 "$OUT/deleted-gc-$c.png"; done
project_import "$P" "$OUT/deleted-gc-red.png" pic > /dev/null && project_import "$P" "$OUT/deleted-gc-blue.png" twin_a > /dev/null \
  && project_import "$P" "$OUT/deleted-gc-blue.png" twin_b > /dev/null \
  && project_api "$P" /assets -X POST -H 'Content-Type: application/json' \
    -d "$(jq -n --arg t "$here/project/draw.py/draw" '{tool: $t, kind: "image", name: "reader", exact: true, fields: {instructions: "a hat on <pic>"}}')" > /dev/null \
  || { check "deleted gc: the boxes weren't made"; undo_window "$lock" ''; return; }
pic=$(asset_of "$P" pic | jq -r .sha256) twin=$(asset_of "$P" twin_b | jq -r .sha256)
in_files "$P" "$pic" && in_files "$P" "$twin" || check "deleted gc: the imports aren't in files/"
# A thumbnail made, as the page asks one: its going is checked below.
project_api "$P" "/thumbs/${pic#sha256:}" -o /dev/null || check "deleted gc: no thumbnail of pic"
[ -n "$(derived "$P" "$pic")" ] || check "deleted gc: pic's thumbnail isn't kept by its hash, so its going proves nothing"

# Deleted: it resolves nowhere. A render of what mentions it, and the CLI's run of it, fail free, naming it.
project_api "$P" /assets/pic -X DELETE > /dev/null || check "deleted gc: delete pic"
[ -z "$(asset_of "$P" pic)" ] || check "deleted gc: pic is still there after its delete"
before=$(project_api "$P" '/runs?limit=100' | jq -c '[.runs[] | select(.kind == "call") | .number]')
project_api "$P" /runs -X POST -H 'Content-Type: application/json' -d '{"names": ["reader"], "on": ["local"]}' > "$OUT/deleted-gc-render.json" 2>&1 || true
for _ in $(seq 100); do said=$(asset_of "$P" reader | jq -r '.failure.message // empty'); [ -z "$said" ] || break; sleep 0.2; done
[[ $said == *'<pic> was deleted'* ]] || check "deleted gc: a render of reader wasn't refused naming <pic> deleted: ${said:-nothing}"
after=$(project_api "$P" '/runs?limit=100' | jq -c '[.runs[] | select(.kind == "call") | .number]')
[ "$before" = "$after" ] || check "deleted gc: the refused render made a run: $before, then $after"
in_project "$P" run "$here/project/draw.py/draw" 'instructions=a hat on <pic>' --on local > "$OUT/deleted-gc-run.txt" 2>&1 || code=$?
[ "$code" != 0 ] && grep -q '<pic> was deleted' "$OUT/deleted-gc-run.txt" \
  || check "deleted gc: cozy run of <pic> wasn't refused naming it deleted (exit $code): $(tail -c 300 "$OUT/deleted-gc-run.txt")"

# Undo in the window: back, with its file.
entry=$(entry_of "$P" 'Deleted pic')
project_history_step "$P" undo "${entry:-0}" > "$OUT/deleted-gc-undo.json" 2>&1 || check "deleted gc: Undo in the window: $(tail -c 300 "$OUT/deleted-gc-undo.json")"
[ "$(asset_of "$P" pic | jq -r .sha256)" = "$pic" ] || check "deleted gc: Undo in the window didn't bring pic back with its file: $(asset_of "$P" pic | jq -c '{name, sha256}')"

# Deleted again, and a box holding the same file as another: past the window, their files leave unless a box holds them.
project_api "$P" /assets/pic -X DELETE > /dev/null && project_api "$P" /assets/twin_a -X DELETE > /dev/null || check "deleted gc: delete pic, twin_a"
began=$SECONDS
for _ in $(seq $((window + 60))); do in_files "$P" "$pic" || break; sleep 1; done
! in_files "$P" "$pic" || check "deleted gc: pic's file is still in files/ $((window + 60)) s after its delete"
[ $((SECONDS - began)) -ge $((window - 2)) ] || check "deleted gc: pic's file went $((SECONDS - began)) s after its delete, inside the $window s window"
for _ in $(seq 50); do [ -n "$(derived "$P" "$pic")" ] || break; sleep 0.2; done
[ -z "$(derived "$P" "$pic")" ] || check "deleted gc: what Calcifer made of pic is kept: $(derived "$P" "$pic" | head -3 | tr '\n' ' ')"
[ "$(sql1 "$P" "SELECT count(*) FROM lock_files WHERE sha256 = '$pic'")" = 0 ] || check "deleted gc: project.db still records pic's file"
in_files "$P" "$twin" || check "deleted gc: twin_b's file went with twin_a's delete"
! project_api "$P" "/thumbs/${pic#sha256:}" -o /dev/null 2> /dev/null || check "deleted gc: pic's thumbnail is still served"

# Undo past the window: the box back without its files, saying so.
entry=$(entry_of "$P" 'Deleted pic')
project_history_step "$P" undo "${entry:-0}" > "$OUT/deleted-gc-bare.json" 2>&1 || true
J '(.lost // []) | any(startswith("pic'"'"'s files were removed (deleted more than "))' "$OUT/deleted-gc-bare.json" \
  || check "deleted gc: Undo past the window doesn't say pic's files were removed: $(head -c 400 "$OUT/deleted-gc-bare.json")"
box_kept "$P" pic || check "deleted gc: Undo past the window: not pic back without its file: $(asset_of "$P" pic | jq -c '{name, sha256, versions}')"
! in_files "$P" "$pic" || check "deleted gc: Undo brought pic's file back"
entry=$(entry_of "$P" 'Deleted twin_a')
project_history_step "$P" undo "${entry:-0}" > "$OUT/deleted-gc-twin.json" 2>&1 || true
J '(.lost // []) | any(startswith("twin_a'"'"'s files were removed"))' "$OUT/deleted-gc-twin.json" \
  || check "deleted gc: twin_a's Undo doesn't say its files were removed: $(head -c 400 "$OUT/deleted-gc-twin.json")"

# Deleted with Calcifer stopped across its window: gone once it starts.
project_import "$P" "$OUT/deleted-gc-green.png" late > /dev/null || check "deleted gc: import late"
late=$(asset_of "$P" late | jq -r .sha256)
project_api "$P" /assets/late -X DELETE > /dev/null || check "deleted gc: delete late"
calcifer_halt || { undo_window "$lock" ''; return; }
sleep $((window + 3))
in_files "$P" "$late" || check "deleted gc: late's file went with Calcifer stopped"
cozy --json machine ls > /dev/null || check "deleted gc: Calcifer didn't start again"
for _ in $(seq 150); do in_files "$P" "$late" || break; sleep 0.2; done
! in_files "$P" "$late" || check "deleted gc: late's file is still there 30 s after Calcifer started past its window"
undo_window "$lock" ''
drop_folder "$P"
}

# cl-919: Undo after the real 3-hour window. Two boxes deleted, a tool's box with its words and a take, and a picture; Calcifer stopped,
# and every delete and history entry of the project dated 3 h 1 min back, as if that long had gone; started again, its open purges both
# boxes' files. Undo then brings each back where it was without them, its tool and words kept, saying so; Redo deletes the bare box again,
# and Undo brings it back again. A picture no one deleted keeps its file.
db_purged() {
step "Undo after the 3-hour purge: each box back without its files, saying so (cl-919)"
local P=$OUT/purged hero pic stays aged rows ops at_hero at_pic prompt='a red hat on a cat' c
run_project "$P" > "$OUT/purged-new.log" 2>&1 || { check "purged: no project: $(tail -c 300 "$OUT/purged-new.log")"; return; }
for c in red blue green; do ffmpeg -v error -y -f lavfi -i "color=c=$c:size=64x48" -frames:v 1 "$OUT/purged-$c.png"; done
project_api "$P" /assets -X POST -H 'Content-Type: application/json' \
  -d "$(jq -n --arg t "$here/project/draw.py/draw" --arg p "$prompt" '{tool: $t, kind: "image", name: "hero", exact: true, fields: {instructions: $p}, at: {x: 480, y: 120}}')" > /dev/null \
  && project_api "$P" /assets/hero/versions -X POST -H 'Content-Type: application/json' -d "$(jq -n --arg s "$(project_upload "$P" "$OUT/purged-red.png")" '{source: $s}')" > /dev/null \
  && project_import "$P" "$OUT/purged-blue.png" pic > /dev/null && project_import "$P" "$OUT/purged-green.png" stays > /dev/null \
  || { check "purged: the boxes weren't made"; return; }
hero=$(asset_of "$P" hero | jq -r .sha256) pic=$(asset_of "$P" pic | jq -r .sha256) stays=$(asset_of "$P" stays | jq -r .sha256)
at_hero=$(place_of "$P" hero) at_pic=$(place_of "$P" pic)
in_files "$P" "$hero" && in_files "$P" "$pic" && in_files "$P" "$stays" || { check "purged: a take isn't in files/"; return; }
project_api "$P" /assets/hero -X DELETE > /dev/null && project_api "$P" /assets/pic -X DELETE > /dev/null || check "purged: the deletes"
in_files "$P" "$hero" && in_files "$P" "$pic" || check "purged: a file went inside the window"
calcifer_halt || return
aged=$(python3 -I - "$P/project.db" $(((3 * 60 + 1) * 60 * 1000)) <<'PY'
import sqlite3, sys
db, ago = sqlite3.connect(sys.argv[1]), int(sys.argv[2])
rows = db.execute("UPDATE lock_assets SET record = json_set(record, '$.deleted_ms', json_extract(record, '$.deleted_ms') - ?) WHERE json_extract(record, '$.deleted_ms') IS NOT NULL", (ago,)).rowcount
ops = db.execute("UPDATE ops SET at_ms = at_ms - ?", (ago,)).rowcount
db.commit()
print(rows, ops)
PY
) || check "purged: project.db couldn't be dated back"
cozy --json machine ls > /dev/null || check "purged: Calcifer didn't start again"
read -r rows ops <<< "$aged"
[ "${rows:-0}" = 2 ] && [ "${ops:-0}" -ge 5 ] || check "purged: not both deletes and the history dated back: ${aged:-nothing}"
status "$P" > /dev/null || check "purged: the project doesn't open past the window"
for _ in $(seq 150); do in_files "$P" "$hero" || in_files "$P" "$pic" || break; sleep 0.2; done
! in_files "$P" "$hero" && ! in_files "$P" "$pic" || check "purged: the open past the window kept a deleted box's file"
in_files "$P" "$stays" || check "purged: the purge took the file of a box no one deleted"
[ -n "$(entry_of "$P" 'Deleted pic')" ] && [ -n "$(entry_of "$P" 'Deleted hero')" ] || check "purged: the history no longer lists both deletes"

# Undo, newest first: pic, then hero.
project_api "$P" /history/undo -X POST -H 'Content-Type: application/json' -d '{}' > "$OUT/purged-pic.json" 2>&1 || true
J '(.lost // []) | any(contains("pic'"'"'s files were removed (deleted more than 3 hours ago)"))' "$OUT/purged-pic.json" \
  || check "purged: Undo doesn't say pic's files were removed: $(head -c 400 "$OUT/purged-pic.json")"
box_kept "$P" pic || check "purged: not pic back without its file: $(asset_of "$P" pic | jq -c '{name, sha256, versions}')"
[ "$(place_of "$P" pic)" = "$at_pic" ] || check "purged: pic back at $(place_of "$P" pic), not $at_pic"
project_api "$P" /history/undo -X POST -H 'Content-Type: application/json' -d '{}' > "$OUT/purged-hero.json" 2>&1 || true
J '(.lost // []) | any(contains("hero'"'"'s files were removed (deleted more than 3 hours ago)"))' "$OUT/purged-hero.json" \
  || check "purged: Undo doesn't say hero's files were removed: $(head -c 400 "$OUT/purged-hero.json")"
box_kept "$P" hero && asset_of "$P" hero | J --arg p "$prompt" '.fields.instructions == $p and (.tool | test("draw"))' \
  || check "purged: hero not back with its tool and words and no take: $(asset_of "$P" hero | jq -c '{tool, fields, sha256, versions}')"
[ "$(place_of "$P" hero)" = "$at_hero" ] || check "purged: hero back at $(place_of "$P" hero), not $at_hero"

# Redo deletes the bare box again; Undo brings it back again.
project_api "$P" /history/redo -X POST -H 'Content-Type: application/json' -d '{}' > /dev/null 2>&1 || true
[ -z "$(asset_of "$P" hero)" ] || check "purged: Redo didn't delete hero again"
project_api "$P" /history/undo -X POST -H 'Content-Type: application/json' -d '{}' > /dev/null 2>&1 || true
asset_of "$P" hero | J --arg p "$prompt" '.fields.instructions == $p' || check "purged: a second Undo didn't bring hero back"
! in_files "$P" "$hero" && ! in_files "$P" "$pic" || check "purged: a file came back"
drop_folder "$P"
}

# cl-823: Calcifer closes a project nothing reads. Projects of 200 boxes each, made by `cozy apply`, opened by the CLI and shown by a
# page's stream (`PUT /v1/events/{stream}`): each is open while a call or a stream holds it (its project.db among Calcifer's open files),
# and closes once the call has answered and once the stream shows another. Calcifer's heap is back within a tenth of where it stood after
# two projects went the same way; a reopen is as quick as the first open; a render keeps its project open while it goes, one waiting for
# its machine too, and closes once it has landed.
db_closes() {
step "projects closed once nothing reads them: by the CLI and a page's stream, memory back, a reopen as quick, a render holding its own (cl-823)"
[ "${RUNNER_OS:-}" = Linux ] || { echo "closes: Calcifer's open files are read on Linux"; return 0; }
CLOSES=$OUT/closes CLOSES_PREV='' CLOSES_OPENS=() CLOSES_REOPENS=() CLOSES_OPEN=() CLOSES_HELD=()
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder n=12 start later code pid first second update run_first look queued=0 held_first=0 held_second=0 P
mkdir -p "$CLOSES"
ffmpeg -v error -y -f lavfi -i 'color=c=gray:size=32x18' -frames:v 1 "$CLOSES/seed.png"
code=$(curl -fsS -X POST -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$(jq -r .address "$lock")/v1/session-codes" | jq -r .code) \
  && CLOSES_SESSION=$(curl -fsS -X POST -H 'Content-Type: application/json' -d "$(jq -n --arg c "$code" '{code: $c}')" "http://$(jq -r .address "$lock")/v1/session" | jq -r .token) \
  || { check "closes: no page session"; return; }
curl -sSN -H "Authorization: Bearer $CLOSES_SESSION" -H 'cozy-project-storage: 3' -H 'Accept: text/event-stream' "http://$(jq -r .address "$lock")/v1/events" > "$CLOSES/page.sse" 2>&1 & pid=$!
for _ in $(seq 100); do CLOSES_STREAM=$(sse "$CLOSES/page.sse" | jq -r 'select(.type == "hello") | .stream' | head -1) && [ -n "$CLOSES_STREAM" ] && break; sleep 0.1; done
[ -n "$CLOSES_STREAM" ] || { check "closes: the page's stream said no hello: $(head -c 300 "$CLOSES/page.sse")"; kill "$pid" 2> /dev/null || true; return; }
closes_pass 1 2 || { kill "$pid" 2> /dev/null || true; return; }
closes_show '' > /dev/null || true; closes_shut "$CLOSES/p$CLOSES_PREV" || CLOSES_OPEN+=("stream:$CLOSES_PREV"); CLOSES_PREV=''
sleep 15
start=$(closes_heap || true)
CLOSES_OPENS=() CLOSES_REOPENS=()
closes_pass 3 $((n + 2)) || { kill "$pid" 2> /dev/null || true; return; }
closes_show '' > /dev/null || true; closes_shut "$CLOSES/p$CLOSES_PREV" || CLOSES_OPEN+=("stream:$CLOSES_PREV")
kill "$pid" 2> /dev/null || true
sleep 60
later=$(closes_heap || true)
local open_median reopen_median
open_median=$(printf '%s\n' "${CLOSES_OPENS[@]}" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }')
reopen_median=$(printf '%s\n' "${CLOSES_REOPENS[@]}" | sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }')
echo "closes: $n projects of 200 boxes; heap $start MiB after two, $later MiB after all; open $open_median s, reopen $reopen_median s (medians)"
[ ${#CLOSES_HELD[@]} = 0 ] || check "closes: a project the stream shows isn't open: ${CLOSES_HELD[*]}"
[ ${#CLOSES_OPEN[@]} = 0 ] || check "closes: still open once nothing read it: ${CLOSES_OPEN[*]}"
awk -v a="$later" -v b="$start" 'BEGIN { exit !(a != "" && b != "" && a <= b * 1.1) }' || check "closes: Calcifer's heap $later MiB, past a tenth over $start MiB after two projects"
awk -v r="$reopen_median" -v o="$open_median" 'BEGIN { exit !(r != "" && o != "" && r <= o * 1.25 + 0.05) }' || check "closes: a reopen ($reopen_median s) slower than the first open ($open_median s)"

# A render keeps its project open with nothing else on it, one waiting for its machine too; each closes once it has landed.
first=$CLOSES/first second=$CLOSES/second
for P in "$first" "$second"; do cozy --json new --unencrypted "$P" > /dev/null 2>&1 || { check "closes: a render's project"; return; }; done
for P in "$first:20" "$second:1"; do
  project_api "${P%:*}" /assets -X POST -H 'Content-Type: application/json' \
    -d "$(jq -n --arg t "$here/project/waits.py/wait" --argjson s "${P##*:}" '{tool: $t, name: "waits", exact: true, fields: {instructions: "waits \($s)", seconds: $s}}')" > /dev/null \
    || { check "closes: a render's box"; return; }
done
closes_shut "$first" && closes_shut "$second" || check "closes: a render's project open before its render"
project_api "$first" /runs -X POST -H 'Content-Type: application/json' -d '{"names": ["waits"], "on": ["local"]}' > /dev/null || { check "closes: the first render refused"; return; }
for _ in $(seq 300); do
  run_first=$(project_api "$first" /runs | jq -r '[.runs[] | select(.kind == "call")][0] // empty | select(.status == "in_progress" and .machine_state == "running") | .number' || true)
  [ -z "$run_first" ] || break; sleep 1
done
[ -n "$run_first" ] || { check "closes: the first render never ran"; return; }
# The machine's update waits for it to end (lifetime.sh's update_waiting), and the second render queues behind both.
(cozy --json machine update local --gpu-memory-limit off -y > "$CLOSES/update.json" 2>&1; echo $? > "$CLOSES/update.code") & update=$!
for _ in $(seq 120); do cozy --json machine show local | J '.status.update // "" | test("^waiting for (other work|run [0-9]+) to end$")' && break; sleep 0.5; done
cozy --json machine show local | J '.status.update // "" | test("^waiting for (other work|run [0-9]+) to end$")' \
  || check "closes: the machine's update never waited for the first render: $(cozy --json machine show local | jq -c .status.update)"
project_api "$second" /runs -X POST -H 'Content-Type: application/json' -d '{"names": ["waits"], "on": ["local"]}' > /dev/null || check "closes: the second render refused"
for _ in $(seq 240); do
  look=$(project_api "$second" /runs | jq -r '[.runs[] | select(.kind == "call")][0] // empty | "\(.status)/\(.machine_state // "")"' || true)
  [[ $look != *queued* ]] || { queued=1; ! closes_open "$second" || held_second=1; }
  ! closes_open "$first" || held_first=1
  fixture_projects | J --arg a "$(id_of "$first")" --arg b "$(id_of "$second")" 'any(.projects[]; (.id == $a or .id == $b) and .busy == true)' || break
  sleep 0.5
done
[ "$queued" = 1 ] || check "closes: the second render never queued behind the first"
[ "$held_first" = 1 ] || check "closes: a render going didn't hold its project open"
[ "$held_second" = 1 ] || check "closes: a render waiting for its machine didn't hold its project open"
for _ in $(seq 300); do kill -0 "$update" 2> /dev/null || break; sleep 1; done
kill "$update" 2> /dev/null || true; wait "$update" 2> /dev/null || true
[ "$(cat "$CLOSES/update.code" 2> /dev/null)" = 0 ] || check "closes: the machine's update did not succeed: $(tail -c 300 "$CLOSES/update.json")"
machine_ready
closes_shut "$first" && closes_shut "$second" || check "closes: a render's project still open once it landed"
drop_folder "$CLOSES"
}
# closes_heap: Calcifer's heap, MiB, as its status says.
closes_heap() { local lock=$HOME/.cozy/calcifer/calcifer.lock.holder; curl -fsS -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$(jq -r .address "$lock")/v1/status" | jq '.heap_bytes / 1048576 * 10 | round / 10'; }
# closes_open <project>: Calcifer has its project.db open. Calcifer is nondumpable: its file descriptors read as root.
closes_open() { sudo -n find "/proc/$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")/fd" -lname "$(realpath "$1")/project.db" 2> /dev/null | grep -q .; }
# closes_shut <project>: it closes within 10 s.
closes_shut() { local _; for _ in $(seq 50); do closes_open "$1" || return 0; sleep 0.2; done; return 1; }
# closes_show <id, or nothing>: what the page on the stream shows now.
closes_show() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder
  curl -fsS -X PUT -H "Authorization: Bearer $CLOSES_SESSION" -H 'cozy-project-storage: 3' -H 'Content-Type: application/json' \
    -d "$(jq -n --arg id "$1" '{projects: (if $id == "" then [] else [$id] end)}')" "http://$(jq -r .address "$lock")/v1/events/$CLOSES_STREAM"
}
# closes_made <i>: project i, 50 pictures of one file and 150 boxes mentioning them, by one `cozy apply`.
closes_made() {
  local P=$CLOSES/p$1 k
  cozy --json new --unencrypted "$P" > "$CLOSES/new-$1.json" 2>&1 || return 1
  cp "$CLOSES/seed.png" "$P/seed.png"
  { for k in $(seq 0 49); do printf 'pic_%s:\n  kind: image\n  takes:\n    - file: seed.png\n' "$k"; done
    for k in $(seq 0 149); do
      printf 'shot_%s:\n  %s:\n    instructions: The colossal blue whale rises through a storm, lit from below, <pic_%s> and <pic_%s>.\n' "$k" "$here/project/draw.py/draw" $((k % 50)) $((k * 7 % 50))
    done
  } > "$P/boxes.yaml"
  in_project "$P" apply -f boxes.yaml > "$CLOSES/apply-$1.txt" 2>&1
}
# closes_status <i>: `cozy status` in project i; its seconds.
closes_status() {
  local t0 t1
  t0=$(date +%s%N); in_project "$CLOSES/p$1" --json status > /dev/null || return 1; t1=$(date +%s%N)
  awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.3f\n", (b - a) / 1e9 }'
}
# closes_pass <first> <last>: each project made, read by the CLI, shown by the stream, which then shows the next, and read again.
closes_pass() {
  local i P id t
  for i in $(seq "$1" "$2"); do
    P=$CLOSES/p$i
    closes_made "$i" || { check "closes: project $i not made: $(tail -c 300 "$CLOSES/apply-$i.txt" "$CLOSES/new-$i.json" 2> /dev/null)"; return 1; }
    closes_shut "$P" || CLOSES_OPEN+=("apply:$i")
    t=$(closes_status "$i") && CLOSES_OPENS+=("$t")
    closes_shut "$P" || CLOSES_OPEN+=("status:$i")
    id=$(id_of "$P" || true)
    closes_shut "$P" || CLOSES_OPEN+=("status:$i")
    closes_show "$id" > "$CLOSES/show-$i.json" || { check "closes: the stream couldn't show project $i: $(cat "$CLOSES/show-$i.json")"; return 1; }
    J --arg id "$id" '.held | index($id)' "$CLOSES/show-$i.json" || check "closes: the stream doesn't hold project $i: $(cat "$CLOSES/show-$i.json")"
    closes_open "$P" || CLOSES_HELD+=("$i")
    if [ -n "$CLOSES_PREV" ]; then
      closes_shut "$CLOSES/p$CLOSES_PREV" || CLOSES_OPEN+=("stream:$CLOSES_PREV")
      t=$(closes_status "$CLOSES_PREV") && CLOSES_REOPENS+=("$t")
      closes_shut "$CLOSES/p$CLOSES_PREV" || CLOSES_OPEN+=("reopen:$CLOSES_PREV")
    fi
    CLOSES_PREV=$i
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
# and once the window ends (config.toml's calcifer.undo_seconds, cut for the cell) the box left deleted loses its files. 0.24.0's own
# cozy, in this one's place, imports two pictures and deletes them as its page does; then this cozy goes back in its place, its Calcifer
# started with the short window, and opens the project for the first time (schema 4 with it).
db_deleted_024() {
step "project.db: boxes deleted under 0.24 keep their files for the undo window from the first 0.25 open; Undo brings one back"
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder undo=40 a b entry old made=0
DB024=$OUT/db-deleted-024
old=$(cozy_0240) || { check "0.24 deletes: 0.24.0's archive"; return; }
cp "$COZY" "$OUT/cozy-now"
in_cozy "$old" && [ "$(cozy --version)" = "cozy 0.24.0" ] || { check "0.24 deletes: 0.24.0 in this cozy's place: $(cozy --version)"; in_cozy "$OUT/cozy-now"; return; }
python3 - "$OUT/db-024-kept.png" "$OUT/db-024-gone.png" <<'PNG'
import struct, sys, zlib
def png(path, rgb):
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    rows = (b"\x00" + bytes(rgb) * 8) * 8
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 8, 8, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
png(sys.argv[1], (200, 40, 40)); png(sys.argv[2], (40, 40, 200))
PNG
a=$(sha "$OUT/db-024-kept.png") b=$(sha "$OUT/db-024-gone.png")
db_024_made || made=$?
undo_window "$lock" "$undo" db_024_back
db_calcifer
[ "$made" = 0 ] || { undo_window "$lock" ''; return; }
[ "$(cozy --version)" = "$("$OUT/cozy-now" --version)" ] || { check "0.24 deletes: this cozy back in its place: $(cozy --version)"; undo_window "$lock" ''; return; }
project_api "$DB024" '' > /dev/null || check "0.24 deletes: first open"
sleep 5
held_024 "$a" || check "0.24 deletes: the first 0.25 open deleted kept_024's file"
held_024 "$b" || check "0.24 deletes: the first 0.25 open deleted gone_024's file"
sql "$DB024" "SELECT record FROM lock_assets WHERE key LIKE '%#%'" | jq -se '[.[] | .[0] | fromjson] | length >= 2 and all(.deleted_ms != null)' > /dev/null \
  || check "0.24 deletes: not both boxes set aside, each with a time, after the first open: $(sql "$DB024" "SELECT key, record FROM lock_assets WHERE key LIKE '%#%'" | head -c 400)"
[ "$(sql1 "$DB024" "SELECT count(*) FROM run_files WHERE run = 0 AND output = 'deleted'")" = 0 ] || check "0.24 deletes: 0.24's deleted rows are still on record"
project_api "$DB024" /history > "$OUT/db-024-history.json" || check "0.24 deletes: history"
entry=$(jq -r '[.entries[] | select((.names // []) | index("kept_024"))][0].id // empty' "$OUT/db-024-history.json")
if [ -z "$entry" ]; then
  check "0.24 deletes: no history entry deletes kept_024: $(jq -c '[.entries[] | {id, words, names}]' "$OUT/db-024-history.json")"
else
  project_history_step "$DB024" undo "$entry" > "$OUT/db-024-undo.json" || check "0.24 deletes: Undo of kept_024: $(tail -c 300 "$OUT/db-024-undo.json")"
  J '.assets | any(.name == "kept_024")' <<< "$(status "$DB024")" || check "0.24 deletes: Undo didn't bring kept_024 back: $(status "$DB024" | jq -c '[.assets[].name]')"
  held_024 "$a" || check "0.24 deletes: kept_024 came back without its file"
fi
for _ in $(seq 1 $((undo + 60))); do held_024 "$b" || break; sleep 1; done
! held_024 "$b" || check "0.24 deletes: gone_024's file is still in files/ $((undo + 60)) s after the first open"
[ -z "$entry" ] || held_024 "$a" || check "0.24 deletes: the purge took kept_024's file, which a box holds again"
undo_window "$lock" ''
}
# held_024 <hex>: a file of that sha256 is in the cell's project's files/.
held_024() { find "$DB024/files" -type f -exec sha256sum {} + 2> /dev/null | grep -q "^$1 "; }
# Under 0.24.0: the project, its two pictures imported and held, then each deleted as its page deletes a box.
db_024_made() {
  local name
  run_project "$DB024" > "$OUT/db-024-new.log" 2>&1 || { check "0.24 deletes: 0.24.0 made no project: $(tail -c 300 "$OUT/db-024-new.log")"; return 1; }
  for name in kept gone; do
    project_import "$DB024" "$OUT/db-024-$name.png" "${name}_024" > "$OUT/db-024-$name-import.json" \
      || { check "0.24 deletes: 0.24.0's import of $name: $(cat "$OUT/db-024-$name-import.json")"; return 1; }
  done
  held_024 "$a" && held_024 "$b" || { check "0.24 deletes: 0.24.0's imports aren't in files/"; return 1; }
  for name in kept gone; do
    project_api "$DB024" "/assets/${name}_024" -X DELETE > "$OUT/db-024-delete.txt" 2>&1 || { check "0.24 deletes: 0.24.0's delete of $name: $(tail -c 300 "$OUT/db-024-delete.txt")"; return 1; }
  done
  held_024 "$a" && held_024 "$b" || { check "0.24 deletes: 0.24.0 deleted a deleted box's file"; return 1; }
  [ "$(sql1 "$DB024" 'PRAGMA user_version')" = 2 ] || { check "0.24 deletes: 0.24.0 made schema $(sql1 "$DB024" 'PRAGMA user_version'), not 2"; return 1; }
}
# This cozy back in its place, as an update puts it, while no Calcifer runs (undo_window's restart).
db_024_back() { state_back && cp "$OUT/cozy-now" "$COZY.new" && chmod +x "$COZY.new" && mv -f "$COZY.new" "$COZY"; }
# cozy_0240: the path of 0.24.0's cozy, from its release archive, fetched once.
cozy_0240() {
  local was=$OUT/cozy-0.24.0 arch old
  old=$(find "$was" -type f -name cozy -print -quit 2> /dev/null)
  if [ -z "$old" ]; then
    arch=$(uname -m); [ "$arch" != arm64 ] || arch=aarch64
    mkdir -p "$was"
    curl -fsSL --retry 3 --retry-all-errors "https://github.com/cozy-creator/cozy-creator/releases/download/v0.24.0/cozy-$arch-unknown-linux-gnu.tar.gz" \
      | tar -xz -C "$was" || return 1
    old=$(find "$was" -type f -name cozy -print -quit)
  fi
  [ -n "$old" ] && echo "$old"
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

# Another program holds project.db, with the exclusive lock SQLite's last connection takes to checkpoint the WAL as it closes, once
# Calcifer has let the project go, and a `cozy run` begins meanwhile (cl-885). Held 7 s, within Calcifer's 10 s wait: the run waits it out
# (schema 3's check of an opening project.db waited rusqlite's own 5 s, so the laptop's second run said "project.db is open in another
# program" while Calcifer's own close checkpointed on a busy disk). Held 20 s, past it: Calcifer says so, and the CLI asks again and says
# what's in the way, as the page waits out a held project.db, until it's let go.
# A keyed write cut off mid-request, sent again under its key, hears what it did, and its key is never in project.db without its answer
# (cl-885): a create, an import and an apply cut off, and those, a patch, rename, copy, delete, undo, redo, unpin, approve and render
# watched for their key's first sight, by HTTP as the page and the CLI send them (keyed.py).
db_keyed() {
step "project.db: a keyed write cut off mid-request hears what it did when sent again; its key is kept with its answer"
COZY=$COZY python3 "$here/keyed.py" "$OUT/db-keyed" "$OUT" > "$OUT/db-keyed.json" 2> "$OUT/db-keyed.err" \
  || check "keyed writes: $(jq -c .failures "$OUT/db-keyed.json" 2> /dev/null) $(tail -c 500 "$OUT/db-keyed.err")"
cat "$OUT/db-keyed.json"
}

db_waits() {
step "project.db held by another program while a cozy run begins: waited out within Calcifer's wait and past it (cl-885)"
local P=$OUT/db-waits hold holder began let_go
cozy --json new --unencrypted "$P" > "$OUT/db-waits-new.json" 2>&1 || { check "db waits: new: $(tail -c 300 "$OUT/db-waits-new.json")"; return; }
in_project "$P" --json run minimax-h3-tools/echo text=first > "$OUT/db-waits-1.ndjson" 2> "$OUT/db-waits-1.err" \
  || { check "db waits: first run: $(tail -c 300 "$OUT/db-waits-1.ndjson") $(tail -c 300 "$OUT/db-waits-1.err")"; return; }
for hold in 7 20; do
# Taken once no connection of Calcifer's is open, and proven exclusive.
python3 - "$P/project.db" "$hold" > "$OUT/db-waits-hold-$hold.txt" 2>&1 <<'HOLD' &
import sqlite3, sys, time
path, hold = sys.argv[1], float(sys.argv[2])
deadline = time.time() + 120
while True:
    db = sqlite3.connect(path, timeout=0, isolation_level=None)
    try:
        db.execute("PRAGMA locking_mode=EXCLUSIVE")
        db.execute("SELECT count(*) FROM meta").fetchall()
        break
    except sqlite3.OperationalError:
        db.close()
        if time.time() > deadline:
            sys.exit(print("never held: Calcifer kept project.db open 120 s after the run"))
        time.sleep(0.2)
try:
    sqlite3.connect(path, timeout=0).execute("PRAGMA user_version").fetchall()
    sys.exit(print("shared: another connection read it"))
except sqlite3.OperationalError:
    print("held", flush=True)
time.sleep(hold)
db.close()
print(f"released {time.time():.3f}", flush=True)
HOLD
holder=$!
for _ in $(seq 1300); do grep -qE '^(held|never|shared)' "$OUT/db-waits-hold-$hold.txt" && break; sleep 0.1; done
grep -qx held "$OUT/db-waits-hold-$hold.txt" || { check "db waits $hold s: no hold: $(cat "$OUT/db-waits-hold-$hold.txt")"; wait "$holder" || true; return; }
began=$(python3 -c 'import time; print(f"{time.time():.3f}")')
in_project "$P" --json run minimax-h3-tools/echo "text=held $hold" > "$OUT/db-waits-$hold.ndjson" 2> "$OUT/db-waits-$hold.err" \
  || check "db waits: a run begun while project.db was held $hold s: $(tail -c 300 "$OUT/db-waits-$hold.ndjson") $(tail -c 300 "$OUT/db-waits-$hold.err")"
wait "$holder" || true
let_go=$(sed -n 's/^released //p' "$OUT/db-waits-hold-$hold.txt")
[ -n "$let_go" ] && python3 -c 'import sys; sys.exit(float(sys.argv[1]) >= float(sys.argv[2]))' "$began" "$let_go" \
  || check "db waits $hold s: the hold ended before the run began: $(cat "$OUT/db-waits-hold-$hold.txt"), run at $began"
J -s 'any(.[]; .type == "run.completed" and .run.status == "completed")' "$OUT/db-waits-$hold.ndjson" \
  || check "db waits: the run begun in a $hold s hold didn't complete: $(tail -c 300 "$OUT/db-waits-$hold.ndjson")"
[ "$hold" -lt 10 ] || grep -q 'project.db is open in another program' "$OUT/db-waits-$hold.err" \
  || check "db waits: held $hold s, the CLI didn't say what it waited for: $(tail -c 300 "$OUT/db-waits-$hold.err")"
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
  api -f -o /dev/null -X PUT --data-binary "@$L/clips/$i.mp4" "http://$ADDR/v1/project-files/sha256:${hex[i]}?project=$ID" || { check "latency: PUT take $i"; return; }
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
[ "$(sql1 "$P" 'PRAGMA user_version')" = "$SCHEMA" ] || { check "CRDT: project.db is not schema $SCHEMA"; return; }
[ "$(sql1 "$P" 'SELECT count(*) FROM crdt_document WHERE format = 1 AND length(snapshot) > 0')" = 1 ] || check "CRDT: no native snapshot"
boxes_in "$P" <<'JSON' || { check "CRDT: seed the editable graph"; return; }
[{"name": "kyogre", "tool": "qwen-image-2/image", "kind": "image", "fields": {"instructions": "A whale."}},
 {"name": "groudon", "tool": "qwen-image-2/image", "kind": "image", "fields": {"instructions": "On shore with <kyogre>."}}]
JSON
tiny "$OUT/crdt-a.png"; cp "$OUT/crdt-a.png" "$OUT/crdt-b.png"; printf 'second' >> "$OUT/crdt-b.png"
project_import "$P" "$OUT/crdt-a.png" art > "$OUT/crdt-import.txt" 2>&1 || check "CRDT: import: $(tail -c 300 "$OUT/crdt-import.txt")"
db_calcifer
source=$(project_upload "$P" "$OUT/crdt-b.png") || check "CRDT: upload second version"
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
# project edit when the next real HTTP edit commits. A read takes the rows only as their last commit left them (cl-885): damaged, it
# reads the document, and the rows' check names the damage.
db_sql "$P" "UPDATE fields SET value = '\"QUERY_CACHE_ONLY\"' WHERE box = 'whale' AND field = 'instructions'; UPDATE ops SET words = 'QUERY_CACHE_ONLY';"
project_api "$P" /integrity > "$OUT/crdt-damage.json" || check "CRDT: the rows' check after projection damage"
J '[.differences[] | select(contains("QUERY_CACHE_ONLY"))] | length > 0' "$OUT/crdt-damage.json" || check "CRDT: projection damage not named: $(cat "$OUT/crdt-damage.json")"
project_api "$P" '' > "$OUT/crdt-damaged-state.json" || check "CRDT: read before a write after projection damage"
! grep -q QUERY_CACHE_ONLY "$OUT/crdt-damaged-state.json" || check "CRDT: a read served damaged projection rows"
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

# Opening a project changes nothing in it (cl-885; the coordinator, 11-12: "opening a project must never change its text"). A starter
# project with a text box, tool boxes with takes whose words mention none of their own and one that does, and an imported picture with
# takes: read, closed and read again, then opened afresh after Calcifer is killed. Its document's heads, every box's fields and place, and
# its history read the same each time.
db_opens() {
step "project.db: a project read, closed and opened again after a restart changes nothing in it"
local O=$OUT/opens c source name
starter_project "$O" > /dev/null 2>&1 || { check "opens: project new"; return; }
db_calcifer
cp "$here/project/draw.py" "$O/"
# Each where a person put it, so where it sits is read too.
jq -n '[{name: "notes", fields: {text: "Keep the boat small."}, at: {x: 100, y: 2000}},
  {name: "drawn", tool: "./draw.py/draw", kind: "image", fields: {instructions: "A lighthouse at dusk, <shore> below."}, at: {x: 500, y: 2000}},
  {name: "again", tool: "./draw.py/draw", kind: "image", fields: {instructions: "<self.1> brighter, the sky clearing."}, at: {x: 900, y: 2000}}]' \
  | boxes_in "$O" || { check "opens: boxes"; return; }
for c in red teal navy; do ffmpeg -v error -y -f lavfi -i "color=c=$c:size=320x180" -frames:v 1 "$O/$c.png"; done
project_import "$O" "$O/red.png" shore > "$OUT/opens-import.txt" 2>&1 || { check "opens: import: $(tail -c 300 "$OUT/opens-import.txt")"; return; }
for name in drawn again shore; do
  for c in teal navy; do
    source=$(project_upload "$O" "$O/$c.png") || check "opens: upload $c"
    project_api "$O" "/assets/$name/versions" -X POST -H 'Content-Type: application/json' -d "{\"source\": \"$source\"}" > "$OUT/opens-version.txt" 2>&1 \
      || check "opens: $name's take from $c: $(tail -c 300 "$OUT/opens-version.txt")"
  done
done
# What it holds: its document's heads, each box's fields, where each sits, and its history.
opens_read() {
  status "$O" | jq -S '{heads: (.heads | sort), fields: ([.assets[] | {(.name): .fields}] | add)}' > "$OUT/opens-$1-state.json" \
    && project_api "$O" /canvas | jq -S .cards > "$OUT/opens-$1-places.json" \
    && project_api "$O" '/history?limit=500' | jq -S '[.entries[].id]' > "$OUT/opens-$1-history.json"
}
opens_read before || { check "opens: read it"; return; }
[ "$(jq 'length' "$OUT/opens-before-places.json")" -ge 3 ] || check "opens: its boxes have no places to compare: $(head -c 300 "$OUT/opens-before-places.json")"
for c in 1 2 3; do status "$O" > /dev/null || check "opens: read $c"; done
opens_read again || { check "opens: read it again"; return; }
kill9
opens_read restarted || { check "opens: read it after a restart"; return; }
db_calcifer
local when part
for when in again restarted; do
  for part in state places history; do
    cmp -s "$OUT/opens-before-$part.json" "$OUT/opens-$when-$part.json" \
      || check "opens: its $part read otherwise $when: $(diff "$OUT/opens-before-$part.json" "$OUT/opens-$when-$part.json" | head -c 800)"
  done
done
}

# A project 0.24.0 made, opened by this cozy (cl-904's schema 3: its files, versions and runs leave the Automerge document for
# project.db's rows on its first open). 0.24.0's own cozy makes it, in this one's place as its release installs it; then this
# cozy goes back in its place, as an update puts it, and opens it: every take, file, word and history entry reads as 0.24.0
# read it, each take with the id it had, and Undo reaches past the upgrade.
db_migrate() {
step "project.db: a project 0.24.0 made opens here with nothing lost"
local M=$OUT/migrate now=$OUT/cozy-now old c source
old=$(cozy_0240) || { check "migrate: 0.24.0's archive"; return; }
cp "$COZY" "$now"
in_cozy "$old" && [ "$(cozy --version)" = "cozy 0.24.0" ] || { check "migrate: 0.24.0 in this cozy's place: $(cozy --version)"; in_cozy "$now"; return; }
echo "migrate: Calcifer $(api "http://$ADDR/v1/status" | jq -r .version) makes it"
mkdir -p "$M"; cp "$here/project/draw.py" "$M/"
# As its page makes boxes (cl-930). A tool box named, with its kind, is made with no machine to read the tool.
jq -n '[{name: "notes", fields: {text: "Notes 0.24.0 wrote."}}, {name: "drawn", tool: "./draw.py/draw", kind: "image", fields: {instructions: "A still."}}]' \
  | boxes_in "$M" || { check "migrate: 0.24.0 made no boxes"; in_cozy "$now"; return; }
for c in red teal navy olive; do ffmpeg -v error -y -f lavfi -i "color=c=$c:size=832x480" -frames:v 1 "$M/$c.png"; done
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
# What a migration must leave as it was: the document's heads, and how many boxes.
local heads_0240 boxes_0240
heads_0240=$(sql1 "$M" 'SELECT heads FROM crdt_document') boxes_0240=$(sql1 "$M" 'SELECT count(*) FROM boxes')
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
[ "$(sql1 "$M" 'PRAGMA user_version')" = "$SCHEMA" ] || check "migrate: project.db is schema $(sql1 "$M" 'PRAGMA user_version') after its first open here, not $SCHEMA"
project_integrity "$M" "its first open here"
[ "$(sql1 "$M" 'SELECT heads FROM crdt_document')" = "$heads_0240" ] || check "migrate: the document's heads moved: $heads_0240, now $(sql1 "$M" 'SELECT heads FROM crdt_document')"
[ "$(sql1 "$M" 'SELECT count(*) FROM boxes')" = "$boxes_0240" ] && [ "$(sql1 "$M" "SELECT count(*) FROM boxes WHERE id = ''")" = 0 ] \
  || check "migrate: $boxes_0240 boxes, now $(sql1 "$M" 'SELECT count(*) FROM boxes'), $(sql1 "$M" "SELECT count(*) FROM boxes WHERE id = ''") with no id"
[ "$(sql1 "$M" "SELECT count(*) FROM meta WHERE key = 'rows_at'")" = 1 ] || check "migrate: its rows don't stand at its document after the migration"
# The project as 0.24.0 left it, kept beside it once.
local kept=$M/project.db.pre-schema$SCHEMA kept_at
[ -f "$kept" ] && [ "$(dump_of "$kept")" = "$was_0240" ] || check "migrate: no copy of it as 0.24.0 left it: $([ -f "$kept" ] && dump_of "$kept") (was $was_0240)"
kept_at=$(stat -c %Y.%s "$kept" 2> /dev/null)
# Calcifer's own log names no project's path: the copy is found beside it.
migrate_read "$M" now || check "migrate: what this cozy holds of it"
local part
for part in facts rows history; do
  cmp -s "$OUT/migrate-0240-$part" "$OUT/migrate-now-$part" || check "migrate: its $part read otherwise here: $(diff "$OUT/migrate-0240-$part" "$OUT/migrate-now-$part" | head -c 800)"
done
[ "$(sql1 "$M" "SELECT count(*) FROM versions WHERE coalesce(json_extract(record, '\$.id'), '') = ''")" = 0 ] || check "migrate: a take has no id"
for f in $(status "$M" | jq -r '.assets[].name' | while read -r name; do held "$M" "$name" | jq -r '.versions[].files[].path // empty'; done); do
  [ -f "$M/$f" ] || check "migrate: $f is gone"
done
# Undo reaches what 0.24.0 did: notes' words back, then changed again. (A box 0.24.0 deleted is cl-919's, dated at the first open.)
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
[ "$(sql1 "$F" 'PRAGMA user_version')" = "$SCHEMA" ] || check "migrate: let be, the copy is schema $(sql1 "$F" 'PRAGMA user_version')"
migrate_facts "$F" | cmp -s - "$OUT/migrate-0240-facts" || check "migrate: let be, the copy reads otherwise: $(migrate_facts "$F" | diff "$OUT/migrate-0240-facts" - | head -c 600)"
[ "$(stat -c %Y.%s "$kept" 2> /dev/null)" = "$kept_at" ] || check "migrate: the copy kept beside it was written again"
[ "$(dump_of "$F/project.db.pre-schema$SCHEMA")" = "$was_dump" ] || check "migrate: the refused copy's own kept copy isn't it as it was"
# The copy kept beside it opens as a project of its own, migrated in its turn, and reads as 0.24.0 read it.
local K=$OUT/migrate-kept
mkdir -p "$K" && cp "$kept" "$K/project.db" && cp -al "$M/files" "$K/files" || check "migrate: a folder for the kept copy"
in_project "$K" --json status > "$OUT/migrate-kept.json" 2>&1 || check "migrate: the kept copy doesn't open: $(tail -c 300 "$OUT/migrate-kept.json")"
project_integrity "$K" "the kept copy's open"
migrate_facts "$K" | cmp -s - "$OUT/migrate-0240-facts" || check "migrate: the kept copy reads otherwise: $(migrate_facts "$K" | diff "$OUT/migrate-0240-facts" - | head -c 600)"
echo "migrate: 0.24.0's project read here as it read there: $(jq -sc 'map({(.name): [.versions[].number]}) | add' "$OUT/migrate-now-facts")"
}

# A 5,000-box project of schema 3 (cl-904's) opens here as $SCHEMA (cl-885) with nothing lost. The migration adds each box's id to its
# row and rows_at and conflicts to meta, changes no row 3 has, and keeps the project as it was beside it once; its rows then stand at
# the document, so a read takes them, and adds the project's run journal beside them. Schema 3 is this cozy's project, Calcifer stopped,
# with what 4 and 5 added taken off.
db_migrate_large() {
step "project.db: a 5,000-box project of schema 3 migrates with nothing lost"
local L=$OUT/migrate-large K=$OUT/migrate-large-kept n=5000 pid was began
cozy --json new --unencrypted "$L" > "$OUT/migrate-large-new.json" 2>&1 || { check "migrate large: project new: $(tail -c 300 "$OUT/migrate-large-new.json")"; return; }
python3 - "$L/nodes.yaml" "$n" <<'PY'
import sys
with open(sys.argv[1], "w") as out:
    for i in range(int(sys.argv[2])):
        out.write(f"note_{i}:\n  kind: text\n  text: Note {i}. The colossal blue whale rises through a storm, lit from below, and the small boat holds.\n")
PY
in_project "$L" apply -f nodes.yaml > "$OUT/migrate-large-apply.txt" 2>&1 || { check "migrate large: apply $n nodes: $(tail -c 300 "$OUT/migrate-large-apply.txt")"; return; }
jq -n --argjson n "$n" '{cards: ([range(0; $n)] | map({key: "note_\(.)", value: {x: ((. % 71) * 300), y: ((. / 71 | floor) * 333)}}) | from_entries)}' > "$OUT/migrate-large-cards.json"
project_api "$L" /canvas -X PATCH -H 'Content-Type: application/json' -d @"$OUT/migrate-large-cards.json" > /dev/null || check "migrate large: place them"
for i in 1 2 3; do
  project_api "$L" "/assets/note_$i" -X PATCH -H 'Content-Type: application/json' -d "{\"fields\": {\"text\": \"Note $i, edited.\"}}" > /dev/null || check "migrate large: edit note_$i"
done
large_read "$L" before || { check "migrate large: what it holds"; return; }
[ "$(jq length "$OUT/migrate-large-before-boxes")" = "$n" ] || check "migrate large: $(jq length "$OUT/migrate-large-before-boxes") boxes, not $n"
pid=$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder")
kill -TERM "$pid"; while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done
python3 - "$L/project.db" <<'PY' || { check "migrate large: made schema 3"; return; }
import sqlite3, sys
db = sqlite3.connect(sys.argv[1], isolation_level=None)
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
journal = ["outputs", "run_logs", "asks", "runs", "project_view", "project_diagnostics", "journal_migrations", "after_steps",
    "project_headers", "project_rendered_here", "project_vault_operations"]
db.executescript("BEGIN; ALTER TABLE boxes DROP COLUMN id; DELETE FROM meta WHERE key IN ('rows_at', 'conflicts'); "
    + "".join(f"DROP TABLE {table}; " for table in journal) + "PRAGMA user_version = 3; COMMIT;")
PY
was=$(dump_of "$L/project.db")
began=$SECONDS
in_project "$L" --json status > "$OUT/migrate-large-open.json" 2>&1 || { check "migrate large: its open: $(tail -c 300 "$OUT/migrate-large-open.json")"; return; }
echo "migrate large: $n boxes migrated and read in $((SECONDS - began)) s"
db_calcifer
[ "$(sql1 "$L" 'PRAGMA user_version')" = "$SCHEMA" ] || check "migrate large: schema $(sql1 "$L" 'PRAGMA user_version') after its open, not $SCHEMA"
local kept=$L/project.db.pre-schema$SCHEMA
[ -f "$kept" ] && [ "$(dump_of "$kept")" = "$was" ] || check "migrate large: no copy of it as it was: $([ -f "$kept" ] && dump_of "$kept") (was $was)"
project_integrity "$L" "its migration"
[ "$(sql1 "$L" "SELECT count(*) FROM meta WHERE key = 'rows_at'")" = 1 ] || check "migrate large: its rows don't stand at its document"
[ "$(sql1 "$L" "SELECT count(*) FROM boxes WHERE id = ''")" = 0 ] || check "migrate large: $(sql1 "$L" "SELECT count(*) FROM boxes WHERE id = ''") boxes with no id"
large_read "$L" after || check "migrate large: what it holds after"
local part
for part in boxes canvas history heads; do
  cmp -s "$OUT/migrate-large-before-$part" "$OUT/migrate-large-after-$part" || check "migrate large: its $part read otherwise: $(diff "$OUT/migrate-large-before-$part" "$OUT/migrate-large-after-$part" | head -c 600)"
done
# The kept copy opens as a project of its own, migrated in its turn, and holds what this did.
mkdir -p "$K" && cp "$kept" "$K/project.db" || check "migrate large: a folder for the kept copy"
in_project "$K" --json status > "$OUT/migrate-large-kept.json" 2>&1 || check "migrate large: the kept copy doesn't open: $(tail -c 300 "$OUT/migrate-large-kept.json")"
project_integrity "$K" "the kept copy's open"
large_read "$K" kept || check "migrate large: what the kept copy holds"
for part in boxes canvas; do
  cmp -s "$OUT/migrate-large-before-$part" "$OUT/migrate-large-kept-$part" || check "migrate large: the kept copy's $part read otherwise"
done
}
# large_read <project> <tag>: each box's fields, the canvas, its history and the document's heads, to $OUT/migrate-large-<tag>-*.
large_read() {
  local state
  state=$(project_api "$1" '') || return
  jq -S '[.assets[] | {name, fields}] | sort_by(.name)' <<< "$state" > "$OUT/migrate-large-$2-boxes" || return
  project_api "$1" /canvas | jq -S '.cards | map_values({x, y, w})' > "$OUT/migrate-large-$2-canvas" || return
  project_api "$1" '/history?limit=200' | jq -c '[.entries[] | {words, undone: (.undone != null)}]' > "$OUT/migrate-large-$2-history" || return
  sql1 "$1" 'SELECT heads FROM crdt_document' > "$OUT/migrate-large-$2-heads"
}
# in_cozy <file>: that cozy in this one's place, as an update puts it, and its Calcifer the one serving: the last stopped as a
# service manager stops one, and started by the next verb. An older cozy refuses this one's newer state.db, as its release
# would never meet it: it keeps one of its own while it serves, and this one's comes back with this cozy ($OUT/cozy-now).
in_cozy() {
  local lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid
  pid=$(jq -r .pid < "$lock" 2> /dev/null) || pid=''
  [ -z "$pid" ] || term_calcifer "$pid" || return
  if [ "$1" -ef "$OUT/cozy-now" ]; then state_back; else state_aside; fi
  cp "$1" "$COZY.new" && chmod +x "$COZY.new" && mv -f "$COZY.new" "$COZY" && cozy --json machine ls > /dev/null && db_calcifer
}
# term_calcifer <pid>: stopped as a service manager stops it; one still running 120 s on is a failure, not a hang.
term_calcifer() {
  local waited=0
  kill -TERM "$1" 2> /dev/null || return 0
  while kill -0 "$1" 2> /dev/null; do
    [ "$waited" -lt 1200 ] || { check "Calcifer $1 still runs 120 s after SIGTERM"; kill -9 "$1" 2> /dev/null; return 1; }
    sleep 0.1; waited=$((waited + 1))
  done
}
# This cozy's state.db set aside while an older cozy serves (once), and back with this cozy.
state_aside() {
  local aside=$OUT/state-aside
  [ ! -d "$aside" ] || return 0
  mkdir -p "$aside" && find "$HOME/.cozy/calcifer" -maxdepth 1 -name 'state.db*' -exec mv -t "$aside" {} +
}
state_back() {
  local aside=$OUT/state-aside
  [ -d "$aside" ] || return 0
  rm -f "$HOME"/.cozy/calcifer/state.db* && find "$aside" -maxdepth 1 -name 'state.db*' -exec mv -t "$HOME/.cozy/calcifer" {} + && rmdir "$aside"
}
# migrate_facts <project>: each box's words, takes (number, approved, files by hash) and discarded, as the API reads them.
migrate_facts() {
  local name
  status "$1" | jq -r '.assets[].name' | sort | while read -r name; do
    held "$1" "$name" | jq -c --arg n "$name" '{name: $n, fields: .asset.fields, approved: .asset.approved, versions: [.versions[] | {number, files: (.files | map_values(.sha256))}], discarded: (.discarded // [])}'
  done
}
# migrate_read <project> <tag>: what it holds, to $OUT/migrate-<tag>-{facts,rows,history} (0.25 has no YAML export, cl-930).
migrate_read() {
  migrate_facts "$1" > "$OUT/migrate-$2-facts" && migrate_rows "$1" > "$OUT/migrate-$2-rows" \
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
