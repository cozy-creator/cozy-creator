"""pickup_kept (cl-1093, #3567's review 5): a render the user asked for reads what it was asked with, through a delete and a purge. `next`
picks up from take 1 of `src` and waits for `hold`, a slow box it mentions; while it waits, `src` is deleted and its undo window (Calcifer
runs with a 20 s one here) ends. The render still goes: next's take picks up from src's take, whose file was kept for it.
    python3 pickup_kept.py <project> <out>: $COZY the cozy under test. Prints what it saw as JSON and exits 1 if anything failed."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import cast

from project_http import project_path, request

Json = dict[str, object]
HERE = Path(__file__).resolve().parent
SHOTS = HERE / "project" / "shots.py"
# shots.py's takes start their second segment here (se-130).
SEAM = 6
# Past the 20 s undo window, with room for the purge after it.
HOLD_S = 50
RUN_S = 900
# How long a take's extras may take to land from its machine.
LANDS_S = 60


def main() -> int:
    project, out = Path(sys.argv[1]), Path(sys.argv[2])
    failures: list[str] = []
    seen: dict[str, object] = {}

    def check(name: str, ok: bool, detail: object = None) -> None:
        seen[name] = True if ok else (detail if detail is not None else False)
        if not ok:
            failures.append(name)

    def cozy(*words: str, at: Path = project) -> subprocess.CompletedProcess[str]:
        return subprocess.run([os.environ["COZY"], *words], cwd=at, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=RUN_S)

    def api(suffix: str, method: str = "GET", body: Json | None = None) -> Json:
        return cast(Json, json.loads(request(project_path(project, suffix), method, body)))  # cast: Calcifer's JSON

    def node(name: str, fn: str, fields: Json) -> None:
        api("/assets", "POST", {"name": name, "tool": f"{SHOTS}/{fn}", "kind": "video", "exact": True, "fields": fields})

    def versions(name: str) -> list[Json]:
        return cast(list[Json], api(f"/assets/{name}")["versions"])

    def files_of(version: Json) -> list[str]:
        """Each file's hash: a version's files say each as {path, sha256}."""
        held = cast(dict[str, object], version.get("files") or {}).values()
        return [str(file.get("sha256")) if isinstance(file, dict) else str(file) for file in held]

    def in_files(sha256: str) -> bool:
        return any((project / "files").glob(f"{sha256.removeprefix('sha256:')}*"))

    def done() -> None:
        print(json.dumps({"failures": failures, "seen": seen}, indent=1, default=str))

    clip = out / "pickup-kept-look.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(clip)], check=True)
    made = cozy("new", str(project), "--unencrypted", at=out)
    check("new", made.returncode == 0, made.stderr[-300:])
    imported = subprocess.run([sys.executable, str(HERE.parent / "page.py"), "import", str(project), str(clip), "--name", "look"], capture_output=True, text=True)
    check("look_imported", imported.returncode == 0, imported.stderr[-300:])
    if failures:
        done()
        return 1

    # src's take 1, which next picks up from.
    node("src", "shoot", {"summary": "From <look>."})
    first = cozy("render", "src", "--on", "local")
    taken = versions("src")
    check("src_take", first.returncode == 0 and len(taken) == 1, first.stderr[-500:])
    # A take's extras (its latents) come from the machine after its answer, never waited for: here once they've landed (e2e 44142655129: read
    # at once, `latents` was still `absent` on the machine, and was in files/ moments later).
    landed = time.monotonic() + LANDS_S
    while taken and not all(in_files(sha) for sha in files_of(taken[0])) and time.monotonic() < landed:
        time.sleep(1)
        taken = versions("src")
    check("src_files_here", bool(taken) and bool(files_of(taken[0])) and all(in_files(sha) for sha in files_of(taken[0])), taken)
    if failures:
        done()
        return 1
    src_files = files_of(taken[0])
    seen["src_files"] = src_files

    # next waits for hold, which takes HOLD_S; it picks up from <src.1>.
    node("hold", "slow", {"seconds": HOLD_S})
    node("next", "shoot", {"summary": "After <hold>, then <look>."})
    render = subprocess.Popen([os.environ["COZY"], "render", "next", "--pickup", f"<src.1>@{SEAM}", "--on", "local"], cwd=project,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    began = time.monotonic()
    # Asked: hold running, next waiting on it.
    while time.monotonic() - began < 120:
        jobs = cast(list[Json], cast(Json, api("?assets=false").get("board") or {}).get("jobs", []))
        if any(job.get("asset") == "hold" and job.get("state") == "running" for job in jobs):
            break
        time.sleep(0.5)
    seen["hold_running_after_s"] = round(time.monotonic() - began, 1)

    # src deleted while next waits, and its window ends before hold is done.
    api("/assets/src", "DELETE")
    deleted_at = time.monotonic()
    while time.monotonic() - deleted_at < 30:
        time.sleep(1)
    seen["src_files_kept_while_waiting"] = [sha for sha in src_files if in_files(sha)]

    stdout, stderr = render.communicate(timeout=RUN_S)
    seen["render"] = {"code": render.returncode, "out": stdout[-1500:], "err": stderr[-1500:]}
    nexts = versions("next")
    last = nexts[-1] if nexts else {}
    seen["next_version"] = last
    check("render_went", render.returncode == 0 and len(nexts) == 1, seen["render"])
    picked = cast(dict[str, object], last.get("pickup") or {})
    check("picked_up", picked.get("take") == "<src.1>" and picked.get("out_frame") == SEAM, last)
    kept_while_waiting = [sha for sha in src_files if sha in cast(list[str], seen["src_files_kept_while_waiting"])]
    check("src_kept_while_waiting", kept_while_waiting == src_files, kept_while_waiting)

    # Its render done, src's files go: the purge waited, it didn't keep them for good.
    gone_by = time.monotonic() + 60
    while time.monotonic() < gone_by and any(in_files(sha) for sha in src_files):
        api("/assets/next")
        time.sleep(2)
    left = [sha for sha in src_files if in_files(sha) and not any(sha in files_of(version) for version in nexts)]
    check("src_gone_after", not left, left)
    done()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
