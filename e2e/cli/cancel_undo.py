"""Cancel a real local project render while its HTTP delete's commit is held."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import sqlite3
import sys
import time
from uuid import uuid4

from project_http import project_path, request


def main() -> int:
    project, output, tool = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
    report: dict = {}

    def status() -> dict:
        return json.loads(request(project_path(project)))

    def versions(state: dict) -> int:
        return next((asset["versions"] for asset in state["assets"] if asset["name"] == "shot"), 0)

    def edit(path: str, method: str, body: dict) -> str:
        return request(project_path(project, path), method, body)

    def render(label: str) -> None:
        answer = edit("/runs", "POST", {"names": ["shot"], "on": ["local"]})
        (output / f"cancel-render-{label}.json").write_text(answer)
        assert json.loads(answer)["added"] == 1, answer

    def capture(name: str) -> dict[str, dict[str, str]]:
        with sqlite3.connect(f"file:{project / 'project.db'}?mode=ro", uri=True) as source:
            with sqlite3.connect(output / name) as target:
                source.backup(target)
            records = [json.loads(row[0]) for row in source.execute("SELECT record FROM versions WHERE key = 'shot' OR key LIKE 'shot#%' ORDER BY number")]
        retained = {version["id"]: version["files"] for version in records}
        assert len(retained) == len(records) and all(retained.values()), records
        for files in retained.values():
            for digest in files.values():
                sha256 = digest.removeprefix("sha256:")
                paths = (project / "files").glob(sha256 + ".*")
                if not any(hashlib.sha256(path.read_bytes()).hexdigest() == sha256 for path in paths):
                    raise RuntimeError(f"a retained original version's media is missing or changed: {digest}")
        return retained

    edit("/assets", "POST", {
        "tool": tool, "name": "shot", "exact": True,
        "fields": {"seconds": 0.01, "instructions": "Before cancellation."},
    })
    for count in range(1, 4):
        edit("/assets/shot", "PATCH", {"fields": {"instructions": f"Prior version {count}."}})
        render(str(count))
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            current = status()
            if versions(current) == count and not current["busy"] and not current.get("queue"):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError(f"prior version {count} did not land: {current}")
    report["before_versions"] = versions(current)
    before_delete = capture("cancel-before-delete.db")
    report["retained_ids_before_delete"] = sorted(before_delete)

    edit("/assets/shot", "PATCH", {"fields": {"seconds": 60}})
    render("long")
    deadline = time.monotonic() + 24
    while time.monotonic() < deadline:
        jobs = status().get("board", {}).get("jobs", [])
        run = next((job["run"] for job in jobs if job["asset"] == "shot" and job.get("run")), None)
        if run and json.loads(request(f"/v1/runs/{run}"))["status"] == "in_progress":
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("the long project render did not start")
    report["run"] = run

    delete_edit = f"cancel-undo-{uuid4().hex}"
    with ThreadPoolExecutor(max_workers=1) as workers:
        lock = sqlite3.connect(project / "project.db", isolation_level=None)
        lock.execute("BEGIN IMMEDIATE")
        deleted = workers.submit(request, project_path(project, "/assets/shot"), "DELETE", edit=delete_edit)
        try:
            # Read scheduling scalars only. The real run must settle before the
            # DELETE can commit. A pending HTTP response alone would not prove it.
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
            if deleted.done():
                raise RuntimeError("delete completed despite the external SQLite write lock")
            # The follower is runnable now, but its project writes still wait for the delete.
            time.sleep(1)
            report["delete_held_until_terminal"] = not deleted.done()
        finally:
            lock.execute("ROLLBACK")
            lock.close()
        answer = deleted.result(timeout=40)
    (output / "cancel-delete.json").write_text(answer)
    assert json.loads(answer)["name"] == "shot", answer

    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        current = status()
        if not current["busy"] and not current.get("queue"):
            break
        time.sleep(0.05)
    else:
        raise RuntimeError("the canceled project's work did not settle")
    assert all(asset["name"] != "shot" for asset in current["assets"]), current["assets"]
    before_undo = capture("cancel-before-undo.db")
    report["retained_ids_before_undo"] = sorted(before_undo)
    history = json.loads(request(project_path(project, "/history")))
    entries = [entry for entry in history["entries"] if delete_edit in entry.get("edits", [])]
    assert len(entries) == 1 and "shot" in entries[0]["names"] and not entries[0].get("undone"), entries
    entry = entries[0]["id"]
    report["undo_entry"] = entry
    undone = edit("/history/undo", "POST", {"entry": entry})
    (output / "cancel-undo.json").write_text(undone)
    assert [item["id"] for item in json.loads(undone)["entries"]] == [entry], undone

    after = status()
    report["after_versions"] = versions(after)
    report["after_queue"] = after.get("queue", [])
    after_undo = capture("cancel-after-undo.db")
    report["retained_ids_after_undo"] = sorted(after_undo)
    report["retained_media_unchanged"] = before_delete == before_undo == after_undo
    report["passed"] = (
        report["before_versions"] == report["after_versions"] == 3
        and report["delete_held_until_terminal"] and not report["after_queue"]
        and len(before_delete) == 3 and report["retained_media_unchanged"]
    )
    print(json.dumps(report))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"inconclusive": str(error)}))
        raise
