# e2e/cli/walheld.sh: a lock while a reader outside Cozy holds the machine journal and state.db (xs-220, hack audit tensord #3 and
# calcifer-cli #7).

# wal_held: an encrypted project runs on local, and Calcifer's state.db holds its id (the machine journal holds only digests of a
# project's runs, cl-984, so its WAL is judged by what TensorD says). A reader outside both (python's sqlite3, one read
# transaction on each) then holds an older snapshot of both while the project locks. The lock never waits on it: it goes while the
# reader still holds, and each store says its WAL's emptying is owed. Once the reader lets go, the next write of each empties its
# WAL: TensorD says so, and neither state.db nor its WAL holds the project's id. The lock tells the person, once, which program holds
# the index open.
wal_held() (
  step "a lock never waits on a reader outside Cozy; the next writes empty the WALs it held (xs-220)"
  [ "${RUNNER_OS:-Linux}" = Linux ] || { echo "wal held: Linux runners only"; exit 0; }
  local P=$OUT/wal-held U=$OUT/wal-after secrets view id journal state holder began took
  secrets=$(mktemp -d)
  trap 'kill "${holder:-}" 2> /dev/null || true; in_project "$P" --json vault lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"' EXIT
  journal=$HOME/.cozy/machine/var/lib/cozy/tensord/execution/executions.sqlite3 state=$HOME/.cozy/calcifer/state.db
  # Calcifer's warnings are kept only in a test's diagnostics log.
  calcifer_diagnostics
  printf '%s\n' 'synthetic wal-held passphrase' > "$secrets/pass" && chmod 600 "$secrets/pass"
  cozy --json new "$P" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$OUT/wal-new.json" 2> "$OUT/wal-new.err" \
    || { check "wal held: new encrypted project: $(tail -c 300 "$OUT/wal-new.err")"; exit 0; }
  view=$(jq -r '.view // empty' "$OUT/wal-new.json") id=$(jq -r '.id // empty' "$OUT/wal-new.json")
  [ -n "$view" ] && [ -d "$view" ] && [ -n "$id" ] || { check "wal held: the encrypted project has no id or unlocked view: $(head -c 300 "$OUT/wal-new.json")"; exit 0; }
  cp "$here/script/words.py" "$view/words.py" && printf '{"text": "held"}\n' > "$view/input.json" || { check "wal held: its script"; exit 0; }
  in_project "$P" --json run "$view/words.py/count" --input "$view/input.json" --on local > "$OUT/wal-run.ndjson" 2>&1
  grep -Eq '"status": ?"completed"' "$OUT/wal-run.ndjson" || { check "wal held: its run didn't complete: $(tail -c 400 "$OUT/wal-run.ndjson")"; exit 0; }
  [ -f "$journal" ] || { check "wal held: no machine journal at $journal"; exit 0; }
  # What state.db holds of it, or this proves nothing there.
  grep -aqsF "$id" "$state" "$state-wal" || { check "wal held: state.db never held the project's id ($(ls -l "$state"* 2>&1 | tr '\n' ' '))"; exit 0; }

  # A reader outside Cozy: one read transaction on each store, held 120 s, past the lock.
  python3 -I - "$journal" "$state" > "$OUT/wal-holder.log" 2>&1 <<'PY' &
import sqlite3, sys, time
held = []
for path in sys.argv[1:]:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True, isolation_level=None)
    db.execute("BEGIN")
    db.execute("SELECT count(*) FROM sqlite_master").fetchone()
    held.append(db)
print("holding", flush=True)
time.sleep(120)
PY
  holder=$!
  for _ in $(seq 100); do grep -q holding "$OUT/wal-holder.log" && break; sleep 0.2; done
  grep -q holding "$OUT/wal-holder.log" || { check "wal held: the reader didn't start: $(tail -c 300 "$OUT/wal-holder.log")"; exit 0; }
  began=$(date +%s)
  in_project "$P" --json vault lock > "$OUT/wal-lock.json" 2>&1 || check "wal held: the lock failed: $(tail -c 300 "$OUT/wal-lock.json")"
  took=$(( $(date +%s) - began ))
  kill -0 "$holder" 2> /dev/null || check "wal held: the lock waited ${took} s, until the outside reader let go"
  # The person is told once, naming the program (python3 here: Linux names the lock's holder).
  grep -q "another program (python3, pid [0-9]*) has this computer's project index open, so the project's id stays in the index's write log until that program closes" "$OUT/wal-lock.json" \
    || check "wal held: the lock didn't say another program holds the index open: $(tail -c 400 "$OUT/wal-lock.json")"
  cozy machine logs local tensord -n 400 > "$OUT/wal-tensord.log" 2>&1
  cp "$HOME/.cozy/calcifer/diagnostics.log" "$OUT/wal-calcifer.log" 2> /dev/null || true
  grep -q "the machine journal: its WAL still holds deleted rows' pages" "$OUT/wal-tensord.log" \
    || check "wal held: TensorD didn't say its WAL's emptying is owed: $(grep -i -E 'wal|journal' "$OUT/wal-tensord.log" | tail -3 | cut -c1-300)"
  grep -q "state.db: its WAL still holds deleted rows' pages" "$OUT/wal-calcifer.log" \
    || check "wal held: Calcifer didn't say state.db's WAL's emptying is owed: $(grep -i -E 'wal|state.db' "$OUT/wal-calcifer.log" | tail -3 | cut -c1-300)"

  # The reader lets go. A second program opens state.db read-write after it, as a SQLite browser would: Calcifer's own locks on
  # the index stayed whole (naming the reader opened none of its files), so nothing rebuilds the index under Calcifer.
  kill "$holder" 2> /dev/null || true; wait "$holder" 2> /dev/null || true; holder=
  python3 -I - "$state" > "$OUT/wal-second.log" 2>&1 <<'PY' || check "wal held: a second program's read-write open of state.db: $(tail -c 300 "$OUT/wal-second.log")"
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
print(db.execute("PRAGMA quick_check").fetchone()[0])
db.close()
PY
  grep -qx ok "$OUT/wal-second.log" || check "wal held: state.db's quick_check from a second program: $(head -c 300 "$OUT/wal-second.log")"
  cozy --json machine ls > "$OUT/wal-status.json" 2>&1 || check "wal held: Calcifer's status after a second program opened its index: $(tail -c 300 "$OUT/wal-status.json")"
  # Later retries wait a second after the last (`Wal::retry`): past it, Calcifer's next write empties state.db's WAL.
  sleep 1.2
  cozy --json new --unencrypted "$U" > "$OUT/wal-after.json" 2>&1 || { check "wal held: a project after: $(tail -c 300 "$OUT/wal-after.json")"; exit 0; }
  # Calcifer keeps only warnings: its fact is the WAL itself, and the database file too.
  ! grep -aqsF "$id" "$state" "$state-wal" || check "wal held: state.db or its WAL still holds the locked project's id after Calcifer's next write"
  # TensorD's next writes: a run.
  printf '{"text": "after"}\n' > "$OUT/wal-after-input.json"
  in_project "$U" --json run "$here/script/words.py/count" --input "$OUT/wal-after-input.json" --on local > "$OUT/wal-after-run.ndjson" 2>&1 \
    || check "wal held: a run after: $(tail -c 300 "$OUT/wal-after-run.ndjson")"
  cozy machine logs local tensord -n 400 > "$OUT/wal-tensord-after.log" 2>&1
  grep -q "the machine journal: its WAL, owed an emptying, is empty" "$OUT/wal-tensord-after.log" \
    || check "wal held: TensorD's next write didn't empty its owed WAL: $(grep -i -E 'wal|journal' "$OUT/wal-tensord-after.log" | tail -3 | cut -c1-300)"
  # Nothing rebuilt or recovered the index under Calcifer, and it still writes.
  ! grep -aiE "recovered [0-9]+ frames|rebuil|disk image is malformed|SQLITE_CORRUPT" "$HOME/.cozy/calcifer/diagnostics.log" 2> /dev/null \
    || check "wal held: Calcifer's diagnostics show its index recovered or damaged: $(grep -aiE 'recovered [0-9]+ frames|rebuil|disk image is malformed|SQLITE_CORRUPT' "$HOME/.cozy/calcifer/diagnostics.log" | tail -2 | cut -c1-300)"
  cozy --json machine ls > "$OUT/wal-status-after.json" 2>&1 || check "wal held: Calcifer's status at the end: $(tail -c 300 "$OUT/wal-status-after.json")"
  echo "wal held: the lock took ${took} s beside the outside reader; the next writes emptied both WALs ($(stat -c %s "$journal-wal" 2> /dev/null || echo 0) and $(stat -c %s "$state-wal" 2> /dev/null || echo 0) bytes)"
)
