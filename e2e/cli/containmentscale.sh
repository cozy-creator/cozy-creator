# containment_scale (dispatch-only, --shard containment-scale): a 0.24 home grown as a laptop's months of agent work leave it,
# SCALE_MAINTENANCE project-less runs on `local` (calls, warms, updates outside any project) with ~100 KB outcomes and
# SCALE_PROJECT runs landing in SCALE_DIRS projects' .runs, each with its logs, and SCALE_UNCODED projects in its index from
# before public codes whose project.db can't be read. 0.24.0's own Calcifer makes the store (another user's home); the rows
# are added to it; this cozy's first start then fences it (v7 -> v8). Timed: that start, `cozy doctor` answering after it (under 2 s);
# with `local` running and an events stream open as a page holds one, `machine show local`, `machine ls` and
# `model ls --on local` 5 times each, under 2 s each (#3270: each read every run on `local`, twice, and the stream again on
# every Status); and 100 reads each of a plain project, a locked encrypted project's vault status and the projects list, whose
# unreadable projects are tried once, never on every list. One JSON line.
containment_scale() {
step "containment scale: a large 0.24 home's first start here, and 100 project reads after it"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "containment scale: a fresh GitHub runner's"; return; }
local S=$OUT/scale user=cozy-scale home old began took took_start doctor secret plain locked i free
local maintenance=${SCALE_MAINTENANCE:-40000} project=${SCALE_PROJECT:-10000} dirs=${SCALE_DIRS:-600} outcome=${SCALE_OUTCOME:-100000} uncoded=${SCALE_UNCODED:-300}
mkdir -p "$S"
# The store and twice its size again for a WAL: fewer runs on a smaller disk, said below.
free=$(df --output=avail -B1 "$S" | tail -1)
while [ $(( (maintenance + project) * outcome * 3 )) -gt "$free" ] && [ "$maintenance" -gt 1000 ]; do maintenance=$((maintenance / 2)) project=$((project / 2)); done
old=$(cozy_0240) || { check "containment scale: 0.24.0's archive"; return; }
id "$user" > /dev/null 2>&1 || sudo useradd -m "$user" || { check "containment scale: no user"; return; }
sudo install -D -m 755 "$old" /opt/cozy-scale/cozy || { check "containment scale: 0.24.0 not installed"; return; }
(cd / && sudo -u "$user" -H /opt/cozy-scale/cozy --json machine ls < /dev/null > "$S/old-ls.json" 2> "$S/old.err") \
  || { check "containment scale: 0.24.0 made no store: $(tail -c 300 "$S/old.err")"; return; }
sudo pkill -KILL -u "$user" || true
home=$(getent passwd "$user" | cut -d: -f6)
sudo sh -c "cp $home/.cozy/calcifer/state.db* '$S/'" && sudo chown "$(id -u):$(id -g)" "$S"/state.db* || { check "containment scale: no 0.24.0 store"; return; }
python3 - "$S/state.db" "$maintenance" "$project" "$dirs" "$outcome" "$S/projects" "$uncoded" "$S/uncoded" > "$S/grown.json" <<'PY' || { check "containment scale: grow the store: $(tail -c 300 "$S/grown.json")"; return; }
import json, sqlite3, sys
path, maintenance, project, dirs, outcome, roots, uncoded, unread = sys.argv[1], *map(int, sys.argv[2:6]), sys.argv[6], int(sys.argv[7]), sys.argv[8]
db = sqlite3.connect(path, isolation_level=None)
assert db.execute("PRAGMA user_version").fetchone()[0] == 7, "not a 0.24 store"
assert db.execute("SELECT 1 FROM machines WHERE name = 'local'").fetchone(), "0.24.0 recorded no local machine"
# An outcome as a finished run's: what it made, and its measurements making up the rest.
made = {"installation": "scale-environment", "models": [{"repository": f"scale/model-{i}", "checkpoint": f"{i:064x}"} for i in range(40)]}
done = {"status": "succeeded", "result": made, "outputs": [], "execution_ms": 1000, "computation_digest": "sha256:00", "measurements": {"observations": []}}
while len(json.dumps(done)) < outcome:
    done["measurements"]["observations"].append({"stage": "denoise", "step": len(done["measurements"]["observations"]), "ms": 12.5, "attention": "sdpa"})
db.execute("PRAGMA synchronous=OFF")
db.execute("BEGIN")
runs = ("WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < :count) "
        "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec, out_dir, state, status, outcome, created_at, settled_at) "
        "SELECT printf('run-%s%020d', :prefix, i), :first + i, printf('scale-%s-%d', :prefix, i), 'sha256:00', 'http://127.0.0.1:1', {kind}, 'scale/runs', {machine}, 0, X'7B7D', {out}, "
        "'settled', 'completed', {outcome}, 1700000000000 + i, 1700000000000 + i FROM n")
# Calcifer's own, as a laptop's agents leave them: calls, warms and updates outside any project, all on this computer's machine.
db.execute(runs.format(kind="CASE i % 4 WHEN 0 THEN 'warm' WHEN 1 THEN 'update' ELSE 'call' END", machine="'local'", out="'/home/runner/.cozy/outputs/scale/' || i", outcome="CAST(:done AS BLOB)"),
           {"count": maintenance, "prefix": "m", "first": 1, "done": json.dumps(done)})
# A project's: its outputs land in the project's .runs.
db.execute(runs.format(kind="'call'", machine="NULL", out=f"'{roots}/p' || (i % {dirs}) || '/.runs'", outcome="randomblob(:size)"), {"count": project, "prefix": "p", "first": maintenance + 1, "size": outcome + 20000})
# Projects the index has known since before public codes, whose project.db can't be read.
for i in range(uncoded):
    db.execute("INSERT INTO projects (id, path, used_at_ms, name) VALUES (?, ?, 1700000000000, ?)", (f"scale-uncoded-{i}", f"{unread}/u{i}", f"uncoded_{i}"))
columns = [row for row in db.execute("PRAGMA table_info(run_logs)")]
filled = {name: ("k" if "INT" in kind.upper() else "'a log line of a scale run'" if "TEXT" in kind.upper() else "X''")
          for _, name, kind, notnull, default, _ in columns if name != "run" and notnull and default is None}
names = ", ".join(["run", *filled]); values = ", ".join(["r.id", *filled.values()])
db.execute(f"INSERT INTO run_logs ({names}) SELECT {values} FROM runs r, (SELECT 1 AS k UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5) WHERE r.target = 'scale/runs'")
db.execute("COMMIT")
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
print(json.dumps({"maintenance": maintenance, "project": project, "dirs": dirs, "outcome": outcome, "uncoded": uncoded,
                  "runs": db.execute("SELECT count(*) FROM runs").fetchone()[0], "logs": db.execute("SELECT count(*) FROM run_logs").fetchone()[0]}))
PY
for i in $(seq 0 $((uncoded - 1))); do mkdir -p "$S/uncoded/u$i" && : > "$S/uncoded/u$i/project.db"; done
# The encrypted project made and locked while this cozy's own store is in place: its lock seals through the local worker
# this store knows. Its folder is named by its path once the grown store replaces that store.
plain=$S/plain locked=$S/locked secret=$S/secret
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$secret"
cozy --json new "$locked" --passphrase-file "$secret" > "$S/locked.json" 2> "$S/locked.err" || check "containment scale: an encrypted project: $(tail -c 300 "$S/locked.err")"
cozy --json vault --directory "$locked" lock > "$S/lock.json" 2>&1 || check "containment scale: lock: $(jq -r '.error.code // empty' "$S/lock.json" 2> /dev/null)"
# This cozy's Calcifer stopped, the grown store in its place, then its first start: the fence, then `cozy doctor`.
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid
pid=$(jq -r .pid < "$lock" 2> /dev/null) || pid=''
systemctl --user stop cozy-calcifer.service 2> /dev/null || true
# Stopped already, as a service manager stops it: under `set -e` a kill of what's gone must not end the shard.
[ -z "$pid" ] || { kill -TERM "$pid" 2> /dev/null || true; while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done; }
rm -f "$HOME"/.cozy/calcifer/state.db*
cp "$S"/state.db "$HOME/.cozy/calcifer/state.db" && chmod 600 "$HOME/.cozy/calcifer/state.db" || { check "containment scale: the grown store not in place"; return; }
# Its warnings kept (tests only, as calcifer_diagnostics keeps them): how often an unreadable project was tried.
local config=$HOME/.cozy/config.toml diagnostics=$HOME/.cozy/calcifer/diagnostics.log
awk '/^diagnostics *=/ { next } { print } /^\[calcifer\]/ { print "diagnostics = true"; put = 1 } END { if (!put) printf "\n[calcifer]\ndiagnostics = true\n" }' "$config" > "$config.next" && mv "$config.next" "$config"
rm -f "$diagnostics"
local bytes; bytes=$(stat -c %s "$HOME/.cozy/calcifer/state.db")
began=$EPOCHREALTIME
cozy --json machine ls > "$S/first.json" 2> "$S/first.err" || check "containment scale: the first start: $(tail -c 300 "$S/first.err")"
took_start=$(python3 -c "print(round($EPOCHREALTIME - $began, 2))")
began=$EPOCHREALTIME
timeout 120 cozy --json doctor > "$S/doctor.json" 2> "$S/doctor.err" || true
doctor=$(python3 -c "print(round($EPOCHREALTIME - $began, 2))")
# Nothing a first command asks waits on a pass over the history (#3270 made it wait for one: 5.7 s here).
python3 -c "import sys; sys.exit($doctor >= 2)" || check "containment scale: cozy doctor after the first start took ${doctor}s, 2 s or more"
local after
after=$(python3 - "$HOME/.cozy/calcifer/state.db" <<'PY' || echo '{}'
import json, os, sqlite3, sys
db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
count = lambda table: db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] if db.execute("SELECT 1 FROM sqlite_schema WHERE name=?", (table,)).fetchone() else None
print(json.dumps({"version": db.execute("PRAGMA user_version").fetchone()[0], "runs": count("runs"), "legacy_runs": count("legacy_project_runs"),
                  "wal_bytes": os.path.getsize(sys.argv[1] + "-wal") if os.path.exists(sys.argv[1] + "-wal") else 0}))
PY
)
# With `local` running and an events stream open as a page holds one: what a person reads of the machine, 5 times each.
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder stream verb run slow=() machine=()
cozy --json machine show local > "$S/local.json" 2>&1 || check "containment scale: local didn't start: $(tail -c 300 "$S/local.json")"
curl -sSN -H "Authorization: Bearer $(jq -r .token "$holder")" -H 'cozy-project-storage: 3' -H 'Accept: text/event-stream' \
  "http://$(jq -r .address "$holder")/v1/events" > "$S/events.sse" 2> "$S/events.err" & stream=$!
for i in $(seq 120); do grep -q '"type":"hello"' "$S/events.sse" && break; sleep 0.5; done
grep -q '"type":"hello"' "$S/events.sse" || check "containment scale: the events stream said no hello in 60 s: $(tail -c 300 "$S/events.err")"
for verb in "machine show local" "machine ls" "model ls --on local"; do
  local most=0
  for run in 1 2 3 4 5; do
    began=$EPOCHREALTIME
    # shellcheck disable=SC2086
    timeout 120 cozy --json $verb > "$S/verb.json" 2>&1 || check "containment scale: cozy $verb: $(tail -c 300 "$S/verb.json")"
    took=$(python3 -c "print(round($EPOCHREALTIME - $began, 2))")
    most=$(python3 -c "print(max($most, $took))")
  done
  machine+=("\"$verb\": $most")
  python3 -c "import sys; sys.exit($most >= 2)" || slow+=("$verb ${most}s")
done
kill -0 "$stream" 2> /dev/null || check "containment scale: the events stream ended under the reads: $(tail -c 300 "$S/events.err")"
kill "$stream" 2> /dev/null || true
[ ${#slow[@]} = 0 ] || check "containment scale: with local running and a stream open, the slowest of 5 took 2 s or more: ${slow[*]}"
# 100 reads of each: a plain project (its import checked on each), the locked encrypted one's vault status (as a page polls
# it: a locked project's status asks the legacy store for its history), the list.
cozy --json new --unencrypted "$plain" > "$S/plain.json" 2>&1 || check "containment scale: a plain project: $(tail -c 300 "$S/plain.json")"
local state
state=$(project_api "$locked" /vault 2> /dev/null | jq -r '.state // empty') || state=''
[ "$state" = locked ] || check "containment scale: the encrypted project reads ${state:-nothing}, not locked"
local reads=() what
for what in project vault list; do
  began=$EPOCHREALTIME
  for i in $(seq 100); do
    case $what in
      project) project_api "$plain" "" > /dev/null ;;
      vault) project_api "$locked" /vault > /dev/null ;;
      list) fixture_projects > /dev/null ;;
    esac || { check "containment scale: $what read $i"; break; }
  done
  reads+=("\"$what\": $(python3 -c "print(round($EPOCHREALTIME - $began, 2))")")
done
# Each unreadable project was tried once, by the first list: not again on the next 99.
local tried
[ -f "$diagnostics" ] || check "containment scale: Calcifer kept no diagnostics log"
tried=$(grep -c 'keeping the existing project link' "$diagnostics" 2> /dev/null) || tried=0
[ "$tried" -ge 1 ] && [ "$tried" -le "$uncoded" ] || check "containment scale: $uncoded unreadable projects were tried $tried times over 100 lists"
local IFS=,
echo "containment scale: {\"cozy\": \"$(cozy --version)\", \"grown\": $(cat "$S/grown.json"), \"state_db_bytes\": $bytes, \"first_start_s\": $took_start, \"doctor_s\": $doctor, \"after\": $after, \"vault_state\": \"${state:-none}\", \"machine_most_s\": {${machine[*]}}, \"uncoded_tried\": $tried, \"reads_100_s\": {${reads[*]}}}"
}
