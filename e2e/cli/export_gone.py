#!/usr/bin/env python3
"""export_gone (cl-1035; coordinator 11-17): an export that can never finish settles. A plain project's run on local made a still
and its latents; the latents are owed again as Calcifer starts (as an export cut off before it landed is), and the machine no longer
has their bytes (let go of once a project copied them in, or collected). The export settles as lost at once: the output's export
failed, its run says why in one line (`cozy run watch N --once`), and nothing starts the machine for it again: stopped, it stays stopped.
Without the fix the machine answered such a read `not_found`, which reads as a pause, and the exporter started it each time it
stopped, for good."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import sqlite3
import subprocess
import time
from typing import cast

import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from project_http import project_path, request  # noqa: E402

KEPT = Path(__file__).resolve().parent / "project" / "kept.py"
START = re.compile(r"starting this computer's machine pid=(\d+)")
HEALTH = Path.home() / ".cozy/calcifer/health.log"
LOCK = Path.home() / ".cozy/calcifer/calcifer.lock.holder"


def cozy(cli: Path, *words: str, cwd: Path | None = None) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run([str(cli), *words], cwd=cwd, capture_output=True, check=False)


def api(project: Path, suffix: str, method: str = "GET", body: dict[str, object] | None = None) -> dict[str, object]:
    return cast(dict[str, object], json.loads(request(project_path(project, suffix), method, body)))  # cast: Calcifer's JSON


def gone(pid: int, seconds: float = 120) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        time.sleep(0.2)
    return False


def starts() -> int:
    """How many times Calcifer has started this computer's machine, by its health log."""
    return len(START.findall(HEALTH.read_text(errors="replace"))) if HEALTH.exists() else 0


def machine_pid() -> int | None:
    for holder in (Path.home() / ".cozy/machine").rglob("agent.lock.holder"):
        try:
            pid = int(json.loads(holder.read_text())["pid"])
            os.kill(pid, 0)
            return pid
        except (OSError, ValueError, KeyError):
            continue
    return None


def stop_machine() -> None:
    """The local machine stopped as lib.sh's machine_stop stops it."""
    pid = machine_pid()
    if pid is not None:
        os.kill(pid, signal.SIGTERM)
        assert gone(pid), f"the local machine {pid} still runs"


def output_row(view: Path, run: str, name: str) -> tuple[int, int | None, int | None, str | None]:
    db = sqlite3.connect(f"file:{view / 'project.db'}?mode=ro", uri=True, timeout=30)
    try:
        return cast(tuple[int, int | None, int | None, str | None],
                    db.execute("SELECT rev, exported_rev, failed_rev, export_error FROM outputs WHERE run = ? AND name = ?", (run, name)).fetchone())
    finally:
        db.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    facts: dict[str, object] = {}

    project = args.work / "plain"
    assert cozy(args.cli, "--json", "new", "--unencrypted", str(project)).returncode == 0, "cozy new --unencrypted failed"
    api(project, "/assets", "POST", {"name": "still", "tool": f"{KEPT}/shot", "kind": "video", "exact": True, "fields": {"size": 4096}})
    made = cozy(args.cli, "--json", "render", "still", "--on", "local", cwd=project)
    assert made.returncode == 0, f"render: {made.stderr[-400:]!r}"
    version = cast(dict[str, object], cast(list[object], api(project, "/assets/still")["versions"])[-1])
    held = cast(dict[str, object], version["files"])["latents"]
    latents = str(held.get("sha256") if isinstance(held, dict) else held).removeprefix("sha256:")
    db = sqlite3.connect(f"file:{project / 'project.db'}?mode=ro", uri=True, timeout=30)
    record = json.loads(db.execute("SELECT record FROM versions WHERE key = 'still' ORDER BY number DESC LIMIT 1").fetchone()[0])
    db.close()
    run, number = str(record["run_id"]), int(record["run"])
    rev, exported, _, _ = output_row(project, run, "latents")
    assert exported == rev, f"the latents never landed: rev {rev}, exported {exported}"

    # Calcifer and the machine stopped; the latents' bytes gone from the machine; their export owed again.
    pid = int(json.loads(LOCK.read_text())["pid"])
    os.kill(pid, signal.SIGTERM)
    assert gone(pid), f"Calcifer {pid} still runs"
    stop_machine()
    removed = [path for root in (project / ".vault-machine", Path.home() / ".cache/tensorfs/blobs") if root.is_dir()
               for path in root.rglob(latents) if path.is_file()]
    for path in removed:
        path.unlink()
    facts["machine_copies_removed"] = len(removed)
    db = sqlite3.connect(str(project / "project.db"), timeout=30, isolation_level=None)
    db.execute("UPDATE outputs SET exported_rev = NULL, failed_rev = NULL, export_error = NULL WHERE run = ? AND name = 'latents'", (run,))
    db.close()

    # Calcifer again; the project opened, its owed export starts and reads from the machine.
    before = starts()
    assert cozy(args.cli, "--json", "machine", "ls").returncode == 0, "Calcifer didn't start again"
    api(project, "/assets/still")
    settled = None
    began = time.monotonic()
    while time.monotonic() - began < 90:
        rev, exported, failed, error = output_row(project, run, "latents")
        if failed == rev:
            settled = round(time.monotonic() - began, 1)
            facts["export_error"] = error
            break
        time.sleep(1)
    facts["settled_s"] = settled
    facts["starts_while_exporting"] = starts() - before

    # Stopped, the machine stays stopped: nothing it owes starts it.
    stop_machine()
    after = starts()
    time.sleep(45)
    facts["starts_after_stop"] = starts() - after
    facts["machine_running_after"] = machine_pid() is not None
    shown = cozy(args.cli, "--json", "run", "watch", str(number), "--once", cwd=project)
    facts["run_show"] = (shown.stdout + shown.stderr).decode(errors="replace")[-1200:]

    (args.out / "export-gone.json").write_text(json.dumps(facts, indent=2))
    print(f"export gone: {json.dumps({k: v for k, v in facts.items() if k != 'run_show'})}", flush=True)
    assert removed, "the machine held no copy of the latents to remove: nothing here proves the export can't finish"
    assert settled is not None, "the export of bytes the machine no longer has never settled"
    assert facts["starts_after_stop"] == 0 and not facts["machine_running_after"], \
        f"the machine was started {facts['starts_after_stop']} time(s) after it stopped, for an export that can't finish"
    assert "no longer has" in cast(str, facts["run_show"]), f"cozy run watch doesn't say why the latents weren't saved: {cast(str, facts['run_show'])[-400:]}"


if __name__ == "__main__":
    main()
