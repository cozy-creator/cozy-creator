"""`cozy render` (cl-956), as an agent runs it: off a terminal, on weightless fixtures (project/shots.py, project/shaped.py). Only what
it names renders (cl-1192, Paul 11-18: "no you can delete this sort of walking logic. We won't have this idea of 'this is out of
date' right now"): a node reading one with no take says to render that first, in one line, and renders nothing; a chain named renders
in order; an input changed and not rendered is read as it stands, and nothing says out of date; `-n`; a render already going is
joined, a killed CLI's render finishes; a failure stops only what reads it; `--pickup`. Nodes are made through the page's routes, as
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
SHOTS, SHAPED, REQUIRED = HERE / "project" / "shots.py", HERE / "project" / "shaped.py", HERE / "project" / "required.py"
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

    # A 4 s clip each shot shoots from, and a chain: film cuts shot_1 and shot_2, shot_1 reads a picture, mara. A shot's video is the clip
    # it mentions, so `dusk`, another clip, makes a take of other bytes.
    clip, dusk = out / "render-look.mp4", out / "render-dusk.mp4"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=24:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(clip)], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "color=c=navy:size=160x90:rate=24:duration=4", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(dusk)], check=True)
    made = cozy("new", str(project), "--unencrypted", at=out)
    if made.code != 0:
        print(json.dumps({"failures": ["new"], "seen": repr(made)}))
        return 1
    imported = subprocess.run([sys.executable, str(HERE.parent / "page.py"), "import", str(project), str(clip), "--name", "look"], capture_output=True, text=True)
    check("look_imported", imported.returncode == 0, imported.stderr[-500:])
    other = subprocess.run([sys.executable, str(HERE.parent / "page.py"), "import", str(project), str(dusk), "--name", "dusk"], capture_output=True, text=True)
    check("dusk_imported", other.returncode == 0, other.stderr[-500:])
    if imported.returncode != 0:
        # Every shot reads <look>: said here, not as the first take the rest can't make.
        print(json.dumps({"failures": failures, "seen": seen}, indent=1, default=str))
        return 1
    node("mara", SHAPED, "picture", {"prompt": "Mara, in a red raincoat."}, "image")
    node("shot_1", SHOTS, "shoot", {"summary": "<mara> on <look>."})
    node("shot_2", SHOTS, "shoot", {"summary": "Then <look>."})
    api("/assets", "POST", {"name": "film", "tool": "cozy/edit", "exact": True, "fields": {"timeline": {"tracks": [{"clips": [{"clip": "<shot_1>"}, {"clip": "<shot_2>"}]}]}}})

    def row(name: str) -> Json:
        return next((asset for asset in cast(list[Json], api("").get("assets", [])) if asset.get("name") == name), {})

    # Nothing renders on another's behalf: a node reading one with no take says to render it first, once, in one line, and renders
    # nothing. A shot (a machine's) and the film (cut here) alike.
    for name, reads, noun in (("shot_1", "mara", "picture"), ("film", "shot_1", "video")):
        alone = cozy("render", name, "--on", "local")
        seen[f"{name}_alone"] = repr(alone)
        line = f"Render {reads} first: it has no {noun} yet"
        check(f"{name}_says_render_{reads}_first", alone.code != 0 and alone.out.count(line) == 1 and line in alone.err
              and not any(versions(other) for other in ("mara", "shot_1", "shot_2", "film")), alone)
    idle()

    # A chain named renders in order, each reading the take the one before made here, each take a line as it lands, then one next step.
    chain = cozy("render", "mara", "shot_1", "shot_2", "film", "--on", "local")
    seen["chain"] = repr(chain)
    film = path_of(chain, "film")
    check("chain_named_renders_in_order", chain.code == 0 and re.search(r"rendering (it|them) first", chain.out) is None
          and all(took(chain, name) == [1] for name in ("mara", "shot_1", "shot_2", "film")), chain)
    check("chain_film_last_with_its_file", film is not None and film.is_absolute() and film.exists() and chain.lines()[-2].startswith("film take 1 "), chain.lines()[-3:])
    check("chain_next_step", nexts(chain) == [f"next: look at {film}; approve with approved: 1 and apply"], nexts(chain))
    check("no_progress_off_a_terminal", "running" not in chain.err and "waits for" not in chain.err, chain.err[-1000:])

    # The same cut again would make the same film: said, and nothing queued (cl-910).
    again = cozy("render", "film")
    seen["again"] = repr(again)
    check("same_cut_again", again.code == 0 and again.out.strip() == "nothing to render: film version 1 already cuts what it reads now, so cutting it again would make the same film", again)

    # -n renders that many new takes, of a node that has one; --json says each take.
    idle()
    had = len(versions("shot_2"))
    new = cozy("--json", "render", "shot_2", "-n", "2", "--on", "local")
    events = [cast(Json, json.loads(line)) for line in new.lines() if line.startswith("{")]
    # A take that landed no number (a stopped one) is said as None, not a crash that hides it.
    numbers = sorted((cast("int | None", cast(Json, event["take"]).get("number")) for event in events if event.get("type") == "take"), key=lambda number: -1 if number is None else number)
    seen["new"] = {"said": repr(new), "numbers": numbers}
    check("n_new_takes", new.code == 0 and numbers == [had + 1, had + 2] and len(versions("shot_2")) == had + 2, seen["new"])
    check("json_ends_rendered", bool(events) and events[-1].get("type") == "rendered", events[-1:])

    # An input changed and not rendered: nothing calls it out of date, and film alone renders, cut on shot_2's newest take as it stands.
    # Its newest take is of dusk; its words then go back to look, unrendered: rendering shot_2 again would make look's bytes.
    idle()
    api("/assets/shot_2", "PATCH", {"fields": {"summary": "Then <dusk>."}})
    on_dusk = cozy("render", "shot_2", "--on", "local")
    newest = cast(Json, cast(list[Json], row("shot_2").get("prefixes", [{}]))[-1].get("main") or {}).get("sha256")
    check("shot_2_on_dusk", on_dusk.code == 0 and newest is not None and newest != row("look").get("sha256"), {"said": repr(on_dusk), "newest": newest})
    idle()
    api("/assets/shot_2", "PATCH", {"fields": {"summary": "Then <look>, at dusk."}})
    idle()
    state = api("")
    shot_2 = row("shot_2")
    seen["changed_input"] = {"stale": state.get("stale"), "shot_2": {key: shot_2.get(key) for key in ("standing", "status", "changed")}}
    check("nothing_out_of_date", "stale" not in state and shot_2.get("standing") == "current" and "changed" not in shot_2
          and not any(asset.get("standing") == "out_of_date" for asset in cast(list[Json], state.get("assets", []))), seen["changed_input"])
    shots, films = len(versions("shot_2")), len(versions("film"))
    only = cozy("render", "film", "--on", "local")
    seen["only"] = repr(only)
    cut = cast(list[Json], row("film").get("prefixes", []))
    check("only_the_named_renders", only.code == 0 and took(only, "film") == [films + 1] and took(only, "shot_2") == [] and took(only, "shot_1") == []
          and len(versions("shot_2")) == shots and "out of date" not in only.out + only.err, only)
    tracks = cast(list[Json], cast(Json, cut[-1].get("timeline") or {}).get("tracks") or [{}]) if cut else [{}]
    clips = cast(list[Json], tracks[0].get("clips", []))
    read = next((piece.get("sha256") for piece in clips if piece.get("clip") == "<shot_2>"), None)
    check("read_as_it_stands", read is not None and read == newest, {"read": read, "newest": newest, "clips": clips})

    # Approved, then its words changed: it stands on its approved take, current, with no mark of what changed.
    api("/approve", "POST", {"name": "shot_1", "version": "1"})
    api("/assets/shot_1", "PATCH", {"fields": {"summary": "<mara> on <look>, at dawn."}})
    approved = row("shot_1")
    check("approved_stands_unmarked", approved.get("standing") == "current" and approved.get("status") == "approved take 1" and "changed" not in approved,
          {key: approved.get(key) for key in ("standing", "status", "changed")})

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

    # Killed, its render finishes with no CLI following it.
    node("alone", SHOTS, "takes", {"seconds": 6})
    lone = started("render", "alone", "--on", "local")
    check("alone_runs", until("alone running", lambda: running("alone")))
    lone.kill()
    lone.wait()
    check("killed_render_finishes", until("alone landed", lambda: len(versions("alone")) == 1 and not running("alone")))

    # A failure stops only what reads it.
    node("bad", SHOTS, "shoot", {"summary": "On <look>.", "refuse": "the fixture's lens cracked"})
    node("reads_bad", SHOTS, "shoot", {"summary": "From <bad>."})
    node("beside", SHAPED, "shot", {"beats": [{"description": "A grey room.", "duration": 1}]})
    failed = cozy("render", "bad", "reads_bad", "beside", "--on", "local")
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

    # A required choice left empty (cl-1066): refused before any machine, in the box's words: choose, its label, its options as the box
    # shows them; never the field's wire name, nor "write".
    node("pick", REQUIRED, "pick", {}, "image")
    unchosen = cozy("render", "pick", "--on", "local")
    said = unchosen.out + unchosen.err
    seen["unchosen"] = repr(unchosen)
    check("required_choice_says_choose", unchosen.code != 0 and "can't render until you choose its Edit (Character swap, Head swap or Style transfer)" in said
          and "Choose pick's Edit" in said and "needs its edit" not in said and "write pick" not in said and not versions("pick"), unchosen)

    print(json.dumps({"failures": failures, "seen": seen}, indent=1, default=str))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
