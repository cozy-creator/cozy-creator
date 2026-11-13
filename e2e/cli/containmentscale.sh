# containment_scale (dispatch-only, --shard containment-scale): a 0.24 home grown as a laptop's months of agent work leave it,
# SCALE_MAINTENANCE project-less runs (calls, warms, updates outside any project) with ~100 KB outcomes and SCALE_PROJECT runs
# landing in SCALE_DIRS projects' .runs, each with its logs. 0.24.0's own Calcifer makes the store (another user's home); the
# rows are added to it; this cozy's first start then fences it (v7 -> v8). Timed: that start, `cozy doctor` answering after
# it, and 100 reads each of a plain project, a locked encrypted project's vault status and the projects list. One JSON line.
containment_scale() {
step "containment scale: a large 0.24 home's first start here, and 100 project reads after it"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "containment scale: a fresh GitHub runner's"; return; }
local S=$OUT/scale user=cozy-scale home old began took doctor secret plain locked i free
local maintenance=${SCALE_MAINTENANCE:-40000} project=${SCALE_PROJECT:-10000} dirs=${SCALE_DIRS:-600} outcome=${SCALE_OUTCOME:-100000}
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
python3 - "$S/state.db" "$maintenance" "$project" "$dirs" "$outcome" "$S/projects" > "$S/grown.json" <<'PY' || { check "containment scale: grow the store: $(tail -c 300 "$S/grown.json")"; return; }
import json, sqlite3, sys
path, maintenance, project, dirs, outcome, roots = sys.argv[1], *map(int, sys.argv[2:6]), sys.argv[6]
db = sqlite3.connect(path, isolation_level=None)
assert db.execute("PRAGMA user_version").fetchone()[0] == 7, "not a 0.24 store"
db.execute("PRAGMA synchronous=OFF")
db.execute("BEGIN")
runs = ("WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < :count) "
        "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, pinned, spec, out_dir, state, status, outcome, created_at, settled_at) "
        "SELECT printf('run-%s%020d', :prefix, i), :first + i, printf('scale-%s-%d', :prefix, i), 'sha256:00', 'http://127.0.0.1:1', {kind}, 'scale/runs', 0, X'7B7D', {out}, "
        "'settled', 'completed', randomblob(:size), 1700000000000 + i, 1700000000000 + i FROM n")
# Calcifer's own, as a laptop's agents leave them: calls, warms and updates outside any project.
db.execute(runs.format(kind="CASE i % 4 WHEN 0 THEN 'warm' WHEN 1 THEN 'update' ELSE 'call' END", out="'/home/runner/.cozy/outputs/scale/' || i"),
           {"count": maintenance, "prefix": "m", "first": 1, "size": outcome})
# A project's: its outputs land in the project's .runs.
db.execute(runs.format(kind="'call'", out=f"'{roots}/p' || (i % {dirs}) || '/.runs'"), {"count": project, "prefix": "p", "first": maintenance + 1, "size": outcome + 20000})
columns = [row for row in db.execute("PRAGMA table_info(run_logs)")]
filled = {name: ("k" if "INT" in kind.upper() else "'a log line of a scale run'" if "TEXT" in kind.upper() else "X''")
          for _, name, kind, notnull, default, _ in columns if name != "run" and notnull and default is None}
names = ", ".join(["run", *filled]); values = ", ".join(["r.id", *filled.values()])
db.execute(f"INSERT INTO run_logs ({names}) SELECT {values} FROM runs r, (SELECT 1 AS k UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5) WHERE r.target = 'scale/runs'")
db.execute("COMMIT")
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
print(json.dumps({"maintenance": maintenance, "project": project, "dirs": dirs, "outcome": outcome,
                  "runs": db.execute("SELECT count(*) FROM runs").fetchone()[0], "logs": db.execute("SELECT count(*) FROM run_logs").fetchone()[0]}))
PY
# This cozy's Calcifer stopped, the grown store in its place, then its first start: the fence, then `cozy doctor`.
local lock=$HOME/.cozy/calcifer/calcifer.lock.holder pid
pid=$(jq -r .pid < "$lock" 2> /dev/null) || pid=''
systemctl --user stop cozy-calcifer.service 2> /dev/null || true
[ -z "$pid" ] || { kill -TERM "$pid" 2> /dev/null; while kill -0 "$pid" 2> /dev/null; do sleep 0.1; done; }
rm -f "$HOME"/.cozy/calcifer/state.db*
cp "$S"/state.db "$HOME/.cozy/calcifer/state.db" && chmod 600 "$HOME/.cozy/calcifer/state.db"
local bytes; bytes=$(stat -c %s "$HOME/.cozy/calcifer/state.db")
began=$EPOCHREALTIME
cozy --json machine ls > "$S/first.json" 2> "$S/first.err" || check "containment scale: the first start: $(tail -c 300 "$S/first.err")"
took=$(python3 -c "print(round($EPOCHREALTIME - $began, 2))")
began=$EPOCHREALTIME
timeout 120 cozy --json doctor > "$S/doctor.json" 2> "$S/doctor.err" || true
doctor=$(python3 -c "print(round($EPOCHREALTIME - $began, 2))")
local after
after=$(python3 - "$HOME/.cozy/calcifer/state.db" <<'PY'
import json, os, sqlite3, sys
db = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
count = lambda table: db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] if db.execute("SELECT 1 FROM sqlite_schema WHERE name=?", (table,)).fetchone() else None
print(json.dumps({"version": db.execute("PRAGMA user_version").fetchone()[0], "runs": count("runs"), "legacy_runs": count("legacy_project_runs"),
                  "wal_bytes": os.path.getsize(sys.argv[1] + "-wal") if os.path.exists(sys.argv[1] + "-wal") else 0}))
PY
)
# 100 reads of each: a plain project (its import checked on each), a locked encrypted one's vault status (as a page polls it), the list.
plain=$S/plain locked=$S/locked secret=$S/secret
cozy --json new --unencrypted "$plain" > "$S/plain.json" 2>&1 || check "containment scale: a plain project: $(tail -c 300 "$S/plain.json")"
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$secret"
cozy --json new "$locked" --passphrase-file "$secret" > "$S/locked.json" 2> "$S/locked.err" || check "containment scale: an encrypted project: $(tail -c 300 "$S/locked.err")"
cozy --json vault --directory "$locked" lock > /dev/null 2>&1 || check "containment scale: lock"
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
local IFS=,
echo "containment scale: {\"cozy\": \"$(cozy --version)\", \"grown\": $(cat "$S/grown.json"), \"state_db_bytes\": $bytes, \"first_start_s\": $took, \"doctor_s\": $doctor, \"after\": $after, \"reads_100_s\": {${reads[*]}}}"
}
