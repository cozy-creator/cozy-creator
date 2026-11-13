"""`cozy render` (cl-956), as an agent runs it: off a terminal, on weightless fixtures (project/shots.py, project/shaped.py). A chain
renders what it reads first; a current node renders nothing; `--new` and `-n`; a render already going is joined, a killed CLI's render
finishes and is rejoined; a failure stops only what reads it; `--outdated`; `--pickup`. Nodes are made through the page's routes, as
the page makes them; every render is the CLI's.
    python3 render.py <project> <out>: $COZY the cozy under test; each call logged to <out>/argv.log for coverage. Prints what it saw as
    JSON and exits 1 if anything failed."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from collections.abc import Callable
from typing import cast

from project_http import project_path, request

Json = dict[str, object]
HERE = Path(__file__).resolve().parent
SHOTS, SHAPED = HERE / "project" / "shots.py", HERE / "project" / "shaped.py"
# The first render here installs its tools' environments.
RUN_S = 900
# shots.py's takes start their second segment here (se-130).
SEAM = 6


class Said:
    """One `cozy` call: its exit code and what it printed."""

    def __init__(self, code: int, out: str, err: str):
        self.code, self.out, self.err = code, out, err

    def lines(self) -> list[str]:
        return self.out.splitlines()

    def __repr__(self) -> str:
        return json.dumps({"code": self.code, "out": self.out[-2000:], "err": self.err[-2000:]})


def main() -> int:
    project, out = Path(sys.argv[1]), Path(sys.argv[2])
    failures: list[str] = []
    seen: dict[str, object] = {}

    def check(name: str, ok: bool, detail: object = None) -> None:
        seen[name] = True if ok else (detail if detail is not None else False)
        if not ok:
            failures.append(name)

    def logged(words: list[str]) -> list[str]:
        with open(out / "argv.log", "a") as log:
            log.write("".join(f"{word}\x1f" for word in words) + "\x1e")
        return [os.environ["COZY"], *words]

    def cozy(*words: str, at: Path = project) -> Said:
        done = subprocess.run(logged(list(words)), cwd=at, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=RUN_S)
        return Said(done.returncode, done.stdout, done.stderr)

    def started(*words: str) -> subprocess.Popen[str]:
        return subprocess.Popen(logged(list(words)), cwd=project, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def ended(process: subprocess.Popen[str]) -> Said:
        stdout, stderr = process.communicate(timeout=RUN_S)
        return Said(process.returncode, stdout, stderr)

    def api(suffix: str, method: str = "GET", body: Json | None = None) -> Json:
        return cast(Json, json.loads(request(project_path(project, suffix), method, body)))  # cast: Calcifer's JSON

    def node(name: str, tool: Path, fn: str, fields: Json, kind: str = "video") -> None:
        api("/assets", "POST", {"name": name, "tool": f"{tool}/{fn}", "kind": kind, "exact": True, "fields": fields})

    def versions(name: str) -> list[Json]:
        return cast(list[Json], api(f"/assets/{name}")["versions"])

    def jobs() -> list[Json]:
        board = cast(Json, api("?assets=false").get("board") or {})
        return cast(list[Json], board.get("jobs", []))

    def until(what: str, ok: Callable[[], bool], seconds: float = RUN_S) -> bool:
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if ok():
                return True
            time.sleep(0.25)
        seen[f"waited for {what}"] = jobs()
        return False

    def running(name: str) -> bool:
        """Its job went to its machine and hasn't ended."""
        return any(job.get("asset") == name and job.get("state") in ("running", "waiting") and job.get("run") for job in jobs())

    def idle() -> bool:
        """Nothing of the project's renders is left to do: what's counted next is this step's alone."""
        return until("the project idle", lambda: not api("?assets=false").get("busy"))

    def took(said: Said, name: str) -> list[int]:
        """The take numbers its lines say landed of `name`."""
        return [int(found) for found in re.findall(rf"^{name} take (\d+) +done in ", said.out, re.M)]

    def path_of(said: Said, name: str) -> Path | None:
        found = re.search(rf"^{name} take \d+ +done in .*?(/\S+)$", said.out, re.M)
        return Path(found.group(1)) if found else None

    def nexts(said: Said) -> list[str]:
        return [line for line in said.lines() if line.startswith("next: ")]

    # A 4 s clip each shot shoots from, and a chain: film cuts shot_1 and shot_2, shot_1 reads a picture, mara.
    clip = out / "render-look.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(clip)], check=True)
    made = cozy("new", str(project), "--unencrypted", at=out)
    if made.code != 0:
        print(json.dumps({"failures": ["new"], "seen": repr(made)}))
        return 1
    imported = subprocess.run([sys.executable, str(HERE.parent / "page.py"), "import", str(project), str(clip), "--name", "look"], capture_output=True, text=True)
    check("look_imported", imported.returncode == 0, imported.stderr[-500:])
    node("mara", SHAPED, "picture", {"prompt": "Mara, in a red raincoat."}, "image")
    node("shot_1", SHOTS, "shoot", {"summary": "<mara> on <look>."})
    node("shot_2", SHOTS, "shoot", {"summary": "Then <look>."})
    api("/assets", "POST", {"name": "film", "tool": "cozy/edit", "exact": True, "fields": {"timeline": {"tracks": [{"clips": [{"clip": "<shot_1>"}, {"clip": "<shot_2>"}]}]}}})

    # A chain: what film reads, and what that reads, renders first, each take a line as it lands, then one next step.
    chain = cozy("render", "film", "--on", "local")
    seen["chain"] = repr(chain)
    film = path_of(chain, "film")
    check("chain_renders_what_it_reads_first", chain.code == 0 and "film reads shot_1 and shot_2, which have no takes: rendering them first" in chain.out
          and "shot_1 reads mara, which has no takes: rendering it first" in chain.out
          and all(took(chain, name) == [1] for name in ("mara", "shot_1", "shot_2", "film")), chain)
    check("chain_film_last_with_its_file", film is not None and film.is_absolute() and film.exists() and chain.lines()[-2].startswith("film take 1 "), chain.lines()[-3:])
    check("chain_next_step", nexts(chain) == [f"next: look at {film}; approve with approved: 1 and apply"], nexts(chain))
    check("no_progress_off_a_terminal", "running" not in chain.err and "waits for" not in chain.err, chain.err[-1000:])

    # Current: nothing to render, said definitely; -n forces nothing.
    again = cozy("render", "film")
    seen["again"] = repr(again)
    check("nothing_to_render", again.code == 0 and again.out.strip() == "nothing to render: film take 1 is current (--new renders it again)", again)
    forced = cozy("render", "shot_1", "-n", "2", "--on", "local")
    check("n_forces_nothing", forced.code == 0 and forced.out.strip() == "nothing to render: shot_1 take 1 is current (--new renders it again)", forced)

    # --new renders a current node, -n of it; --json says each take.
    idle()
    had = len(versions("shot_2"))
    new = cozy("--json", "render", "shot_2", "--new", "-n", "2", "--on", "local")
    events = [cast(Json, json.loads(line)) for line in new.lines() if line.startswith("{")]
    # A take that landed no number (a stopped one) is said as None, not a crash that hides it.
    numbers = sorted((cast("int | None", cast(Json, event["take"]).get("number")) for event in events if event.get("type") == "take"), key=lambda number: -1 if number is None else number)
    seen["new"] = {"said": repr(new), "numbers": numbers}
    check("new_with_n", new.code == 0 and numbers == [had + 1, had + 2] and len(versions("shot_2")) == had + 2, seen["new"])
    check("json_ends_rendered", bool(events) and events[-1].get("type") == "rendered", events[-1:])

    # Out of date: shot_2's words changed, film renders it first, then itself. Film is cut on shot_2's newest take first (one already cut
    # on it is said current and makes nothing), so it stands on a take its words no longer make: what a cut's own key can't see.
    idle()
    recut = cozy("render", "film", "--new", "--on", "local")
    check("film_cut_on_the_newest", recut.code == 0, recut)
    idle()
    api("/assets/shot_2", "PATCH", {"fields": {"summary": "Then <look>, at dusk."}})
    idle()
    shots, films = len(versions("shot_2")), len(versions("film"))
    stale = cozy("render", "film", "--on", "local")
    seen["stale"] = repr(stale)
    check("out_of_date_renders_first", stale.code == 0 and "film reads shot_2, which is out of date: rendering it first" in stale.out
          and took(stale, "shot_2") == [shots + 1] and took(stale, "film") == [films + 1] and took(stale, "shot_1") == [], stale)

    # Approved stands, unless --outdated.
    api("/approve", "POST", {"name": "shot_1", "version": "1"})
    api("/assets/shot_1", "PATCH", {"fields": {"summary": "<mara> on <look>, at dawn."}})
    kept = cozy("render", "shot_1", "--on", "local")
    check("approved_stands", kept.code == 0 and kept.out.startswith("nothing to render: shot_1 take 1 is approved, though") and "--outdated renders it again" in kept.out, kept)
    outdated = cozy("render", "shot_1", "--outdated", "--on", "local")
    seen["outdated"] = repr(outdated)
    held = api("/assets/shot_1")
    check("outdated_renders_the_approved", outdated.code == 0 and took(outdated, "shot_1") == [2] and cast(Json, held["asset"]).get("approved") == 1, outdated)

    # Joined: a second render of a node rendering joins it, and one killed leaves it going; it lands once.
    node("wait", SHOTS, "takes", {"seconds": 12})
    first = started("render", "wait", "--on", "local")
    check("wait_runs", until("wait running", lambda: running("wait")))
    second = started("render", "wait", "--on", "local")
    first.kill()
    first.wait()
    joined = ended(second)
    seen["joined"] = repr(joined)
    check("rerun_joins", joined.code == 0 and "wait is rendering: joined" in joined.out and took(joined, "wait") == [1], joined)
    check("joined_pays_once", len(versions("wait")) == 1, len(versions("wait")))

    # Killed, its render finishes with no CLI following it; run again, it's current.
    node("alone", SHOTS, "takes", {"seconds": 6})
    lone = started("render", "alone", "--on", "local")
    check("alone_runs", until("alone running", lambda: running("alone")))
    lone.kill()
    lone.wait()
    check("killed_render_finishes", until("alone landed", lambda: len(versions("alone")) == 1 and not running("alone")))
    rejoined = cozy("render", "alone")
    check("killed_then_current", rejoined.code == 0 and rejoined.out.strip() == "nothing to render: alone take 1 is current (--new renders it again)", rejoined)

    # A failure stops only what reads it.
    node("bad", SHOTS, "shoot", {"summary": "On <look>.", "refuse": "the fixture's lens cracked"})
    node("reads_bad", SHOTS, "shoot", {"summary": "From <bad>."})
    node("beside", SHAPED, "shot", {"beats": [{"description": "A grey room.", "duration": 1}]})
    failed = cozy("render", "reads_bad", "beside", "--on", "local")
    seen["failed"] = repr(failed)
    check("failure_stops_its_readers", failed.code != 0 and re.search(r"^bad\b.*failed", failed.out, re.M) is not None
          and re.search(r"^reads_bad\b.*failed", failed.out, re.M) is not None and took(failed, "beside") == [1] and not versions("reads_bad"), failed)

    # Pick up from here: take 1 of shot_2 kept to its seam, the rest rendered again.
    idle()
    shots = len(versions("shot_2"))
    pickup = cozy("render", "shot_2", "--pickup", f"<shot_2.1>@{SEAM}", "--on", "local")
    seen["pickup"] = repr(pickup)
    last = versions("shot_2")[-1]
    check("pickup", pickup.code == 0 and took(pickup, "shot_2") == [shots + 1]
          and last.get("pickup") == {"take": "<shot_2.1>", "out_frame": SEAM, "beats_from": SEAM / 24}, {"said": repr(pickup), "version": last})
    two = cozy("render", "shot_1", "shot_2", "--pickup", "<shot_2.1>@6")
    check("pickup_is_for_one", two.code == 2 and "--pickup is for one node" in two.err, two)

    print(json.dumps({"failures": failures, "seen": seen}, indent=1, default=str))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
