"""A real CLI/SQLite interleaving: cancel a render while a delete's commit is held."""
from __future__ import annotations

import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import time


def main() -> int:
    project, run, output, binary = Path(sys.argv[1]), int(sys.argv[2]), Path(sys.argv[3]), sys.argv[4]
    report: dict = {"run": run}

    def cozy(*args: str) -> str:
        return subprocess.check_output([binary, *args], text=True, stderr=subprocess.STDOUT, timeout=40)

    def status() -> dict:
        return json.loads(cozy("--json", "project", "-p", str(project), "status"))

    def capture(name: str) -> None:
        with sqlite3.connect(f"file:{project / 'project.db'}?mode=ro", uri=True) as source:
            with sqlite3.connect(output / name) as target:
                source.backup(target)

    before = status()
    report["before_versions"] = next(asset["versions"] for asset in before["assets"] if asset["name"] == "shot")
    lock = sqlite3.connect(project / "project.db", isolation_level=None)
    lock.execute("BEGIN IMMEDIATE")
    deleted = subprocess.Popen([binary, "project", "-p", str(project), "delete", "shot"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        # The real machine must acknowledge cancellation while the project's delete still
        # cannot commit. Read scheduling scalars only; do not modify Calcifer's journal.
        with sqlite3.connect(f"file:{Path.home() / '.cozy/calcifer/state.db'}?mode=ro", uri=True) as journal:
            deadline = time.monotonic() + 7
            while time.monotonic() < deadline:
                row = journal.execute("SELECT state, status FROM runs WHERE number = ?", (run,)).fetchone()
                if row == ("settled", "canceled"):
                    report["run_terminal_while_delete_held"] = list(row)
                    break
                time.sleep(0.02)
            else:
                raise RuntimeError("the real run did not settle canceled within the held-commit interval")
        if deleted.poll() is not None:
            raise RuntimeError("delete completed despite the external SQLite write lock")
        # The follower is runnable now, but its project writes still wait for this delete.
        time.sleep(1)
        report["delete_held_until_terminal"] = deleted.poll() is None
    finally:
        lock.execute("ROLLBACK")
        lock.close()
    stdout, stderr = deleted.communicate(timeout=40)
    (output / "cancel-delete.txt").write_text(stdout + stderr)
    if deleted.returncode:
        raise RuntimeError(f"delete failed: {stdout} {stderr}")
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        current = status()
        if not current.get("busy") and not current.get("queue"):
            break
        time.sleep(0.05)
    else:
        raise RuntimeError("the canceled project's work did not settle")
    capture("cancel-before-undo.db")
    entry = stdout.strip().split()[-1]
    report["undo_entry"] = entry
    (output / "cancel-undo.txt").write_text(cozy("project", "-p", str(project), "undo", entry))
    after = status()
    report["after_versions"] = next(asset["versions"] for asset in after["assets"] if asset["name"] == "shot")
    report["after_queue"] = after.get("queue", [])
    capture("cancel-after-undo.db")
    report["passed"] = report["before_versions"] == report["after_versions"] == 3 and not report["after_queue"]
    print(json.dumps(report))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"inconclusive": str(error)}))
        raise
