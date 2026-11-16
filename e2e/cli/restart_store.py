"""state_restart (statedb.sh, cl-1010): a store as a laptop's months of agents leave it, and the routes timed as Calcifer restarts on it.

  fill <state.db> <outputs dir> --runs N --machines M   N settled project-less runs over 120 days (calls, warms, updates on
                                                         `local`, an outcome of ~14 KB on average with its measurements, one in
                                                         10,000 ~2 MB, six log lines each, a call's output exported) and M ended
                                                         rentals, half of them 100 days old; --unsettled K runs left going on `local`
  empty <state.db>                                      those rows out again
  measure ...                                           each route asked every quarter second from the new Calcifer's record on:
                                                         one JSON line per route, then the verdict
"""
import argparse, dataclasses, http.client, json, sqlite3, sys, threading, time

PREFIX = "run-cl1010"


def step(i: int) -> dict[str, object]:
    return {"stage": "denoise", "step": i, "ms": 12.5, "attention": "sdpa"}


def outcome(size: int) -> str:
    """A finished run's outcome of about `size` bytes: what it made, and its measurements making up the rest."""
    steps = [step(i) for i in range(size // len(json.dumps(step(size))) + 1)]
    return json.dumps({"status": "succeeded", "result": {}, "outputs": [], "execution_ms": 1000, "computation_digest": "sha256:00",
                       "measurements": {"observations": steps}})


def fill(args: argparse.Namespace) -> None:
    db = sqlite3.connect(args.db, isolation_level=None)
    assert db.execute("SELECT 1 FROM machines WHERE name = 'local'").fetchone(), "no local machine recorded"
    now = int(time.time() * 1000)
    first = (db.execute("SELECT max(number) FROM runs").fetchone()[0] or 0) + 1
    db.execute("PRAGMA synchronous=OFF")
    db.execute("BEGIN")
    # Over the last 120 days, oldest first, as numbers go up with time.
    db.execute(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < :count) "
        "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec, out_dir, state, status, outcome, "
        "created_at, accepted_at, started_at, settled_at) "
        "SELECT printf('" + PREFIX + "%014d', i), :first + i, printf('cl1010-%d', i), 'sha256:00', 'http://127.0.0.1:1', "
        "CASE i % 4 WHEN 0 THEN 'warm' WHEN 1 THEN 'update' ELSE 'call' END, 'e2e/restart/call', 'local', 0, X'7B7D', :out || '/' || i, "
        "'settled', 'completed', CAST(CASE WHEN i % 10000 = 7 THEN :huge WHEN i % 10 = 0 THEN :big ELSE :done END AS BLOB), "
        ":now - (:count - i) * :gap, :now - (:count - i) * :gap + 1000, :now - (:count - i) * :gap + 1500, :now - (:count - i) * :gap + 9000 FROM n",
        {"count": args.runs, "first": first, "out": args.outputs, "huge": outcome(2_000_000), "big": outcome(60_000), "done": outcome(9_000),
         "now": now, "gap": 120 * 86_400_000 // args.runs})
    columns = list(db.execute("PRAGMA table_info(run_logs)"))
    filled = {name: ("k" if "INT" in kind.upper() else "'step ' || k || ': ' || hex(zeroblob(75))" if "TEXT" in kind.upper() else "X''")
              for _, name, kind, notnull, default, _ in columns if name not in ("run", "level") and notnull and default is None}
    names = ", ".join(["run", "level", *filled]); values = ", ".join(["r.id", "'info'", *filled.values()])
    db.execute(f"INSERT INTO run_logs ({names}) SELECT {values} FROM runs r, (SELECT 1 AS k UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 "
               f"UNION ALL SELECT 5 UNION ALL SELECT 6) WHERE r.id LIKE '{PREFIX}%'")
    db.execute("INSERT INTO outputs (run, name, idx, rev, length, digest, media_type, done, path, exported_rev, created_seq, seq) "
               "SELECT id, 'image', 0, 1, 1000, 'sha256:' || printf('%064d', number), 'image/png', 1, out_dir || '/image.png', 1, 1, 1 "
               f"FROM runs WHERE id LIKE '{PREFIX}%' AND kind = 'call'")
    # Runs an earlier Calcifer left going on `local`, which the machine no longer has: resume takes each up, and each settles lost.
    unsettled = db.execute("SELECT max(number) FROM runs").fetchone()[0] + 1
    db.execute(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < :count) "
        "INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec, out_dir, state, machine_number, "
        "created_at, accepted_at, started_at) "
        "SELECT printf('" + PREFIX + "u%013d', i), :first + i, printf('cl1010-u-%d', i), 'sha256:00', 'http://127.0.0.1:1', 'call', "
        "'e2e/restart/call', 'local', 0, X'7B7D', :out || '/u' || i, 'accepted', 900000 + i, :now - 60000, :now - 59000, :now - 59000 FROM n",
        {"count": args.unsettled, "first": unsettled, "out": args.outputs, "now": now})
    db.execute(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < :count) "
        "INSERT INTO machines (name, kind, hub, rental_id, key, state, served, reason, created_at) "
        "SELECT printf('cl1010-gone-%05d', i), 'rental', 'http://127.0.0.1:1', printf('cl1010-rental-%05d', i), "
        "(SELECT key FROM machines WHERE name = 'local'), 'ended', 0, 'ended', :now - (CASE WHEN i % 2 = 0 THEN 100 ELSE 1 END) * 86400000 - i * 1000 FROM n",
        {"count": args.machines, "now": now})
    db.execute("COMMIT")
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    pages, size = (db.execute(f"PRAGMA {p}").fetchone()[0] for p in ("page_count", "page_size"))
    print(json.dumps({"unsettled": [unsettled, unsettled + args.unsettled - 1], "runs": db.execute("SELECT count(*) FROM runs").fetchone()[0], "logs": db.execute("SELECT count(*) FROM run_logs").fetchone()[0],
                      "machines": db.execute("SELECT count(*) FROM machines").fetchone()[0], "bytes": pages * size}))


def empty(args: argparse.Namespace) -> None:
    db = sqlite3.connect(args.db, isolation_level=None)
    db.execute("BEGIN")
    for table, column in (("outputs", "run"), ("run_logs", "run"), ("runs", "id")):
        db.execute(f"DELETE FROM {table} WHERE {column} LIKE '{PREFIX}%'")
    db.execute("DELETE FROM machines WHERE name LIKE 'cl1010-gone-%'")
    db.execute("COMMIT")
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")


@dataclasses.dataclass
class Asked:
    route: str
    at_ms: int
    ms: float
    status: int | str


@dataclasses.dataclass
class Route:
    route: str
    asked: int = 0
    max_ms: float = 0.0
    statuses: list[int | str] = dataclasses.field(default_factory=list)


def new_record(path: str, old_pid: int) -> dict[str, object]:
    """The lock record of a Calcifer other than `old_pid`, once it lands."""
    began = time.monotonic()
    while time.monotonic() - began < 120:
        try:
            with open(path) as file:
                holder: dict[str, object] = json.load(file)
            if holder["pid"] != old_pid:
                return holder
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.02)
    sys.exit("no new Calcifer's record 120 s on")


def measure(args: argparse.Namespace) -> None:
    holder = new_record(args.holder, args.old_pid)
    recorded = time.monotonic()
    host, port = str(holder["address"]).rsplit(":", 1)
    headers = {"Authorization": f"Bearer {holder['token']}", "cozy-project-storage": "3"}
    routes = [("GET", "/v1/machines"), ("GET", "/v1/tools"), ("POST", "/v1/machines/local/keepalive"),
              ("GET", f"/v1/projects/{args.project}/address"), ("GET", "/v1/runs?limit=50"), ("GET", "/v1/events"),
              ("GET", f"/v1/projects/{args.project}/runs?limit=50"), ("GET", f"/v1/runs/{args.run}")]
    timed: list[Asked] = []
    lock, threads = threading.Lock(), []

    def ask(method: str, path: str) -> None:
        asked = time.monotonic()
        status: int | str
        try:
            conn = http.client.HTTPConnection(host, int(port), timeout=120)
            conn.request(method, path, headers=headers)
            status = conn.getresponse().status  # a stream's answer is its head
            conn.close()
        except OSError as e:
            status = type(e).__name__
        with lock:
            route = path.split('?')[0].replace(args.project, '{id}').replace(f"/v1/runs/{args.run}", "/v1/runs/{id}")
            timed.append(Asked(f"{method} {route}", round((asked - recorded) * 1000),
                               round((time.monotonic() - asked) * 1000, 1), status))

    while time.monotonic() - recorded < args.seconds:
        for method, path in routes:
            thread = threading.Thread(target=ask, args=(method, path), daemon=True)
            thread.start()
            threads.append(thread)
        time.sleep(0.25)
    for thread in threads:
        thread.join(130)
    # This start's closed health log: its steps and any request said slow.
    with open(args.log, errors="replace") as file:
        lines = file.read().splitlines()
    starts = [i for i, line in enumerate(lines) if f'pid={holder["pid"]}' in line and 'event="started"' in line]
    mine = lines[max(starts[-1] - 6, 0):] if starts else []
    steps = {line.split('step="')[1].split('"')[0]: int(line.rsplit("ms=", 1)[1].split()[0]) for line in mine if 'started step="' in line}
    slow = [line.split("INFO ", 1)[-1] for line in mine if "slow request" in line]
    worst: dict[str, Route] = {}
    for t in sorted(timed, key=lambda t: t.at_ms):
        r = worst.setdefault(t.route, Route(t.route))
        r.asked += 1
        r.max_ms = max(r.max_ms, t.ms)
        if t.status not in r.statuses:
            r.statuses.append(t.status)
    for r in worst.values():
        print(json.dumps(dataclasses.asdict(r)))
    # Answered is what's timed: a machine still reattaching may refuse a keepalive, at once. No answer at all is over.
    unanswered = [r.route for r in worst.values() if any(not isinstance(s, int) or s >= 500 for s in r.statuses)]
    slow_routes = [r.route for r in worst.values() if r.max_ms > args.budget_ms]
    over = unanswered + slow_routes
    # Resume ran in this start, beside what was asked: its step is said.
    if "resume" not in steps:
        over.append('resume: no `started step="resume"` in this start\'s health log')
    verdict = {"budget_ms": args.budget_ms, "over": over, "slow_requests": slow, "steps": steps,
               "first_asked_ms": min((t.at_ms for t in timed), default=None)}
    print(json.dumps(verdict))
    with open(args.timed, "w") as file:
        json.dump([dataclasses.asdict(t) for t in timed], file)
    sys.exit(1 if over or slow else 0)


parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest="verb", required=True)
p = sub.add_parser("fill"); p.add_argument("db"); p.add_argument("outputs"); p.add_argument("--runs", type=int, default=40_000); p.add_argument("--machines", type=int, default=3_000); p.add_argument("--unsettled", type=int, default=50)
p = sub.add_parser("empty"); p.add_argument("db")
p = sub.add_parser("measure"); p.add_argument("--holder", required=True); p.add_argument("--old-pid", type=int, required=True); p.add_argument("--project", required=True)
p.add_argument("--log", required=True); p.add_argument("--run", required=True); p.add_argument("--timed", required=True); p.add_argument("--seconds", type=float, default=6); p.add_argument("--budget-ms", type=float, default=200)
args = parser.parse_args()
{"fill": fill, "empty": empty, "measure": measure}[args.verb](args)
