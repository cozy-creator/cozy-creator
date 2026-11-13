# e2e/cli/oldtakes.sh: a project the last release made keeps every take under this cozy (cl-945).

# old_takes: cozy 0.24.0 runs as another user of this runner in its own default home (as machineset.sh's stand-ins do), with its
# own Calcifer and local machine. It makes a project, renders two takes of a weightless picture tool, and stops. The project is
# then this user's, as an update leaves a person's projects. This cozy opens it, and it keeps every take: each number, the id 0.24.0
# gave it, and its file. A take asked with an id the page made (Runs.takes) lands beside them as the version of that id, and
# asked again with that id it isn't made twice.
OLD_RELEASE=0.24.0 OLD_RUNTIME=0.45.0 OLD_TENSORFS=0.24.1
old_takes() {
step "old takes (cl-945): a cozy $OLD_RELEASE project keeps every take under this cozy, and a new take lands by its id"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "old takes: $OLD_RELEASE runs as another user of a fresh GitHub runner"; return; }
local user=cozy-old home P Q take row i
id "$user" > /dev/null 2>&1 || sudo useradd -m "$user" || { check "old takes: no user"; return; }
home=$(getent passwd "$user" | cut -d: -f6)
P=$home/old-takes Q=$OUT/old-takes
mkdir -p "$OUT/cozy-old"
curl -fsSL --retry 3 -o "$OUT/cozy-old.tgz" "https://github.com/cozy-creator/cozy-creator/releases/download/v$OLD_RELEASE/cozy-x86_64-unknown-linux-gnu.tar.gz" \
  && tar -xzf "$OUT/cozy-old.tgz" -C "$OUT/cozy-old" || { check "old takes: no cozy $OLD_RELEASE"; return; }
sudo install -D -m 755 "$OUT/cozy-old/cozy-x86_64-unknown-linux-gnu/cozy" /opt/cozy-old/cozy && sudo install -D -m 644 "$here/project/draw.py" /opt/cozy-old/draw.py \
  || { check "old takes: not installed for its user"; return; }
[ "$(old --version)" = "cozy $OLD_RELEASE" ] || { check "old takes: it says $(old --version)"; return; }
old --json machine show local > "$OUT/old-local.json" || { check "old takes: machine show local: $(tail -c 300 "$OUT/cozy-old.err")"; return; }
old --json machine update local --runtime "$OLD_RUNTIME" --tensorfs "$OLD_TENSORFS" -y > "$OUT/old-update.json" \
  || { check "old takes: its machine: $(tail -c 400 "$OUT/old-update.json") $(tail -c 300 "$OUT/cozy-old.err")"; return; }
old --json new "$P" > "$OUT/old-new.json" || { check "old takes: new: $(tail -c 300 "$OUT/cozy-old.err")"; return; }
old_api "$P" /assets -X POST -H 'Content-Type: application/json' \
  -d "$(jq -nc --arg words "A still of one colour $RANDOM." '{name: "card", exact: true, tool: "/opt/cozy-old/draw.py/draw", kind: "image", fields: {instructions: $words}}')" \
  > "$OUT/old-box.json" 2>&1 || { check "old takes: its box: $(tail -c 300 "$OUT/old-box.json")"; return; }
for i in 1 2; do
  old_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: old-takes-$i-$RANDOM$RANDOM" -d '{"names":["card"],"on":["local"]}' > "$OUT/old-render-$i.json" 2>&1 \
    || { check "old takes: render $i: $(tail -c 300 "$OUT/old-render-$i.json")"; return; }
  old_landed "$P" "$i" || { check "old takes: take $i never landed: $(old_api "$P" "" | jq -c '.assets[] | select(.name == "card") | {versions, making, status, failure}')"; return; }
done
sudo pkill -KILL -u "$user" || true
# Its takes as 0.24.0 kept them: number, id and file, from its project.db's rows.
sudo cp -a "$P" "$Q" && sudo chown -R "$(id -u):$(id -g)" "$Q" || { check "old takes: not handed over"; return; }
old_rows "$Q" > "$OUT/old-takes-before.json" || { check "old takes: its project.db: $(cat "$OUT/old-takes-before.json")"; return; }
J 'length == 2 and all(.id | test("^project-[0-9a-f]{32}$"))' "$OUT/old-takes-before.json" || { check "old takes: 0.24.0 kept $(cat "$OUT/old-takes-before.json")"; return; }

# This cozy: every take, as it was, and its file here.
project_api "$Q" "" > "$OUT/old-takes-open.json" 2> "$OUT/old-takes-open.err" || { check "old takes: open: $(head -c 300 "$OUT/old-takes-open.json") $(tail -c 300 "$OUT/old-takes-open.err")"; return; }
jq -c '[.assets[] | select(.name == "card") | .prefixes[] | {number, id, sha256: .main.sha256, absent: (.main.absent // false)}]' "$OUT/old-takes-open.json" > "$OUT/old-takes-after.json"
jq -e --slurpfile before "$OUT/old-takes-before.json" '[.[] | {number, id, sha256}] == ($before[0] | map({number, id, sha256})) and all(.absent | not)' "$OUT/old-takes-after.json" > /dev/null \
  || check "old takes: this cozy lists $(cat "$OUT/old-takes-after.json"), 0.24.0 kept $(cat "$OUT/old-takes-before.json")"
for sha in $(jq -r '.[].sha256 | sub("^sha256:"; "")' "$OUT/old-takes-before.json"); do
  ls "$Q"/files/"$sha".* > /dev/null 2>&1 || check "old takes: take file $sha is gone"
done
# Its tool where this user keeps it, as a person points a box at their own copy.
cp "$here/project/draw.py" "$OUT/old-draw.py"
project_api "$Q" /assets/card -X PATCH -H 'Content-Type: application/json' -d "{\"tool\":\"$OUT/old-draw.py/draw\"}" > "$OUT/old-tool.json" 2>&1 \
  || { check "old takes: its tool here: $(tail -c 300 "$OUT/old-tool.json")"; return; }

# A take asked with the page's id lands beside them as the version of that id; asked again with it, nothing more is made.
take=project-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
for i in 1 2; do
  project_api "$Q" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: old-takes-new-$i-$RANDOM$RANDOM" \
    -d "{\"names\":[\"card\"],\"on\":[\"local\"],\"takes\":[\"$take\"]}" > "$OUT/old-takes-new-$i.json" 2>&1 || { check "old takes: render by id ($i): $(tail -c 300 "$OUT/old-takes-new-$i.json")"; return; }
done
row=''
for _ in $(seq 300); do
  row=$(project_api "$Q" "" | jq -c '.assets[] | select(.name == "card") | {versions, takes, prefixes: [.prefixes[] | {number, id}]}')
  J '.versions >= 3 and ((.takes // []) | length) == 0' <<< "$row" && break
  sleep 1
done
J --arg take "$take" '.versions == 3 and .prefixes[2] == {number: 3, id: $take}' <<< "$row" || check "old takes: the take by id: $row"
project_api "$Q" "" | jq -c '[.assets[] | select(.name == "card") | .prefixes[0:2][] | {number, id}]' > "$OUT/old-takes-kept.json"
jq -e --slurpfile before "$OUT/old-takes-before.json" '. == ($before[0] | map({number, id}))' "$OUT/old-takes-kept.json" > /dev/null \
  || check "old takes: after a new take it lists $(cat "$OUT/old-takes-kept.json")"
echo "old takes: kept $(cat "$OUT/old-takes-after.json"), new $row"
}

# old <cozy args...>: cozy 0.24.0, as its user, from / (its detached Calcifer keeps the folder it started in).
old() { (cd / && sudo -u cozy-old -H env ${SSL_CERT_FILE:+SSL_CERT_FILE="$SSL_CERT_FILE"} ${COZY_HUB_URL:+COZY_HUB_URL="$COZY_HUB_URL"} /opt/cozy-old/cozy "$@" < /dev/null 2>> "$OUT/cozy-old.err"); }

# old_api <folder> <suffix> <curl args...>: 0.24.0's Calcifer's project API, as project_api is this cozy's.
old_api() {
  local directory=$1 suffix=$2 holder encoded
  shift 2
  holder=$(sudo cat "$(getent passwd cozy-old | cut -d: -f6)/.cozy/calcifer/calcifer.lock.holder") || return
  encoded=$(python3 -c 'from urllib.parse import quote; import sys; print(quote(sys.argv[1], safe=""))' "$directory")
  curl --fail-with-body -sS -H "Authorization: Bearer $(jq -r .token <<< "$holder")" "$@" "http://$(jq -r .address <<< "$holder")/v1/projects/$encoded$suffix"
}

# old_landed <folder> <n>: its box `card` has n takes, none being made, within 5 minutes (the first builds its tool's environment).
old_landed() {
  local row
  for _ in $(seq 300); do
    row=$(old_api "$1" "" 2> /dev/null | jq -c '.assets[] | select(.name == "card")') || row=''
    J --argjson n "$2" '.versions == $n and ((.making // []) | length) == 0' <<< "$row" 2> /dev/null && return 0
    J '.failure != null and ((.making // []) | length) == 0' <<< "$row" 2> /dev/null && return 1
    sleep 1
  done
  return 1
}

# old_rows <folder>: each version of `card` its project.db keeps, read only: number, id, its main file's hash.
old_rows() {
  python3 - "$1/project.db" <<'PYROWS'
import json, sqlite3, sys
with sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True) as db:
    rows = db.execute("SELECT number, record FROM versions WHERE key = 'card' AND discarded = 0 ORDER BY number").fetchall()
kept = []
for number, record in rows:
    version = json.loads(record)
    files = version.get("files") or {}
    main = files.get("image") or next(iter(files.values()), None)
    kept.append({"number": number, "id": version.get("id", ""), "sha256": main})
print(json.dumps(kept))
PYROWS
}
