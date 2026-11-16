"""A project's run numbers (cl-1075), as Paul's lab3_2 kept them on 11-16: schema 6, its runs numbered as Calcifer's one store numbered
them (46550..), each number also kept beside what its run made.
    python3 numbered.py fixture <project.db> <project id> [bad]: makes it so, Calcifer stopped; `bad` adds a take naming one run by its
        number and another by its id, which no migration can move.
    python3 numbered.py moved <project.db>: exits 1, saying what differs, unless it is numbered 1..4 in the order made, each number moved.
    python3 numbered.py read <project.db>: what it keeps of its runs, as JSON.
    python3 numbered.py kept <project.db> <JSON read before>: exits 1 unless it is schema 6 and keeps them as it did."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import sys

T = 1700000000000
RUN = [f"run-numbered{i:018d}" for i in range(1, 5)]
# Made in this order; an old import numbered the second and third as the store did, after the first had its number here.
RUNS = [(RUN[0], 47826, T), (RUN[1], 46550, T + 1), (RUN[2], 46551, T + 2), (RUN[3], 47827, T + 3)]
SHA = "sha256:" + "0" * 64


def file(run: int, run_id: str | None = None) -> dict:
    held = {"run": run, "function": "legacy/runs", "output": "out", "sha256": SHA}
    return held | ({"run_id": run_id} if run_id else {})


def fixture(path: str, project: str, bad: bool) -> None:
    db = sqlite3.connect(path, isolation_level=None)
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    done = json.dumps({"status": "succeeded", "result": {}, "outputs": [], "execution_ms": 1000, "measurements": {"observations": []}})
    landing = str(Path(path).parent / ".runs")
    db.execute("BEGIN")
    for i, (run, number, made) in enumerate(RUNS):
        # The last is no project's, as an old import could leave one: the project then takes no walk up, so it reads as the open left it.
        db.execute("INSERT INTO runs (id, number, idempotency_key, digest, hub, kind, target, machine, pinned, spec, out_dir, state, status, outcome, "
                   "created_at, settled_at, owner) VALUES (?, ?, ?, 'sha256:00', 'http://127.0.0.1:1', 'call', 'legacy/runs', NULL, 0, X'7B7D', ?, "
                   "'settled', 'completed', CAST(? AS BLOB), ?, ?, ?)", (run, number, f"numbered-{i}", landing, done, made, made, project if i < 3 else ""))
    db.execute("INSERT INTO lock_assets (key, record) VALUES ('pic', ?)", (json.dumps({"name": "pic", "kind": "image", "numbered": 6, "ended": [
        {"id": "pic-failed", "run": 46551, "error": {"code": "failed", "message": "it failed"}}]}),))
    versions = [
        {"id": "pic-1", "number": 1, "run": 46550, "run_id": RUN[1]},
        {"id": "pic-2", "number": 2, "run": 47826},
        # A run that isn't here: kept as it is, as no run here is numbered so.
        {"id": "pic-3", "number": 3, "run": 28545, "run_id": "run-gone"},
        # Nor here, numbered as a run here will be: it goes, and the version keeps no run.
        {"id": "pic-4", "number": 4, "run": 3},
    ]
    if bad:
        versions.append({"id": "pic-5", "number": 5, "run": 46550, "run_id": RUN[2]})
    for version in versions:
        db.execute("INSERT INTO versions (key, number, discarded, record) VALUES ('pic', ?, 0, ?)", (version["number"], json.dumps(version)))
    for ord, held in enumerate([file(46551, RUN[2]), file(47827)]):
        db.execute("INSERT INTO lock_runs (ord, record) VALUES (?, ?)", (ord, json.dumps(held)))
        db.execute("INSERT INTO run_files (ord, run, function, output, sha256) VALUES (?, ?, ?, ?, ?)", (ord, held["run"], held["function"], held["output"], SHA))
    walk = {"pending": {"pic-6": {"asset": "pic", "version": {"id": "pic-6", "number": 6}, "run": 47826, "target": {"on": [], "hub": ""}}},
            "walk": {"runs": {"47827": None}, "landing": [file(46550, RUN[1])]}}
    db.execute("INSERT OR REPLACE INTO walk (id, record) VALUES (1, ?)", (json.dumps(walk),))
    db.execute("INSERT INTO request_keys (key, digest, status, content_type, body, at_ms) VALUES ('numbered', 'd', 200, 'application/json', ?, ?)",
               (json.dumps({"run": {"number": 46550, "name": "NUM-46550"}}).encode(), T))
    db.execute("DELETE FROM meta WHERE key = 'key_review'")
    db.execute("PRAGMA user_version = 6")
    db.execute("COMMIT")
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)")


def read(path: str) -> dict:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    one = lambda query: db.execute(query).fetchone()[0]
    walk = json.loads(one("SELECT record FROM walk WHERE id = 1"))
    return {
        "version": one("PRAGMA user_version"),
        "runs": dict(db.execute("SELECT id, number FROM runs ORDER BY id")),
        "versions": {str(n): json.loads(record).get("run") for n, record in db.execute("SELECT number, record FROM versions WHERE key = 'pic' ORDER BY number")},
        "lock_runs": [json.loads(record)["run"] for (record,) in db.execute("SELECT record FROM lock_runs ORDER BY ord")],
        "run_files": [run for (run,) in db.execute("SELECT run FROM run_files ORDER BY ord")],
        "pending": walk["pending"]["pic-6"]["run"],
        "walk_runs": sorted(walk["walk"]["runs"]),
        "landing": [held["run"] for held in walk["walk"]["landing"]],
        "ended": [take.get("run") for take in json.loads(one("SELECT record FROM lock_assets WHERE key = 'pic'")).get("ended", [])],
        "request_keys": one("SELECT count(*) FROM request_keys WHERE key = 'numbered'"),
    }


MOVED = {
    "version": 7,
    "runs": {RUN[0]: 1, RUN[1]: 2, RUN[2]: 3, RUN[3]: 4},
    "versions": {"1": 2, "2": 1, "3": 28545, "4": None},
    "lock_runs": [3, 4],
    "run_files": [3, 4],
    "pending": 1,
    "walk_runs": ["4"],
    "landing": [2],
    "ended": [3],
    "request_keys": 0,
}


def differs(now: dict, wanted: dict) -> dict:
    return {key: {"is": now.get(key), "not": value} for key, value in wanted.items() if now.get(key) != value}


def main() -> int:
    mode, path = sys.argv[1], sys.argv[2]
    if mode == "fixture":
        fixture(path, sys.argv[3], sys.argv[4:] == ["bad"])
        return 0
    now = read(path)
    if mode == "read":
        print(json.dumps(now))
        return 0
    wanted = MOVED if mode == "moved" else json.loads(sys.argv[3]) | {"version": 6}
    wrong = differs(now, wanted)
    if wrong:
        print(json.dumps(wrong))
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
