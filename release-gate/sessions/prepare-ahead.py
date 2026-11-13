"""prepare-ahead's timeline (cl-905): the picture's run, the video's fetch ahead and the video's run, each in the machine's three lines,
seconds from the ask. From the runs' records (runs.json, run-<n>.json), the board every 5 s (board.ndjson) and the pod's TensorD log:
each install turn (`install line: …`, by package), and each object's start and landing (`fetching`, `landed`) with its order, whose
class says whose it is: a run's own (`next`, the picture's the first), a fetch ahead's (`ahead`), a guess (`guess`), or a started run's
call (the video's segment); at order 0, a started call's emergency (cl-966: a read of a model not here, `emergency: …`). A project's
run says nothing in TensorD's log (#2858): its install turns are the board's stages, and its emergencies the downloader's floor
(`floor moved`, which also says each install's hold). The picture's stages are its run's own measures. Prints the timeline as JSON,
end to end against the longest single chain; each check that failed on stderr, and exit 1 when one did. `--compare` prints arms'
timelines side by side.

    prepare-ahead.py <out> <ask unix ms> <picture's package> <video's package>
    prepare-ahead.py --compare <arm dir>…
"""

import json
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import TypeAlias, cast

Json: TypeAlias = str | int | float | bool | None | list["Json"] | dict[str, "Json"]
Row: TypeAlias = dict[str, Json]
# An order's class (TensorD's `origin::priority`) and job: ("run", n), ("ahead", n), ("guess", n), or ("call", 0) for a started run's.
Key: TypeAlias = tuple[str, int]



def compare(dirs: list[Path]) -> None:
    """Each arm's headline numbers in one table: the same row from every arm's timeline.json."""
    def at(t: Json, *path: str) -> Json:
        for key in path:
            t = t.get(key) if isinstance(t, dict) else None
        return t
    rows = {
        "end to end": ("seconds", "end_to_end"), "picture chain": ("seconds", "picture_chain"), "video run": ("seconds", "video_run"),
        "video sent to first call": ("seconds", "video_sent_to_first_call"),
        "picture first weight byte": ("seconds_from_ask", "picture", "download", "start"),
        "picture download end": ("seconds_from_ask", "picture", "download", "end"),
        "picture download MB/s": ("seconds_from_ask", "picture", "download", "mb_s"),
        "picture install": ("seconds_from_ask", "picture", "install"), "picture on the GPU": ("seconds_from_ask", "picture", "gpu"),
        "rewriter load (upsampling prompt / loading model)": ("picture_stages", "upsampling prompt / loading model"),
        "expander load (loading expander)": ("picture_stages", "loading expander"), "upsampling prompt": ("picture_stages", "upsampling prompt"),
        "video fetch ahead": ("seconds_from_ask", "video_ahead", "download"), "guesses": ("seconds_from_ask", "video_ahead", "guesses"),
        "emergencies": ("emergencies",), "floors": ("floors",),
    }
    arms = {d.name.removeprefix("arm-"): cast(Json, json.loads((d / "timeline.json").read_text())) if (d / "timeline.json").exists() else None
            for d in dirs}
    print(json.dumps({name: {arm: at(t, *path) for arm, t in arms.items()} for name, path in rows.items()}, indent=1))


if sys.argv[1] == "--compare":
    compare([Path(d) for d in sys.argv[2:]])
    sys.exit(0)
out, ask, picture_tool, video_tool = Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
LOGGED = re.compile(r"^(\S+)\s+INFO\s+\S+: (fetching|landed) (.*)$")
TURNS = re.compile(r"^(\S+)\s+INFO\s+\S+: install line: (.*?)\s+run=\"?([^\s\"]+)\"?(.*)$")
CLASSES = ("run", "ahead", "guess")
EMERGENCY = re.compile(r"^(\S+)\s+INFO\s+\S+: emergency: (.+?) is (read now|here)\b(.*)$")
FLOOR = re.compile(r"^(\S+)\s+INFO\s+\S+: floor moved (.*)$")
INSTALLING = re.compile(r"installing (\S+) \S+ on this machine")
EARLY = 60_000  # what the log says from before the ask: the update's own


def ms(stamp: str) -> int:
    return int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)


def num(value: Json) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def text(value: Json) -> str:
    return value if isinstance(value, str) else ""


def since(at: int | None) -> float | None:
    return None if at is None else round((at - ask) / 1000, 1)


def load(name: str) -> Json:
    try:
        return cast(Json, json.loads((out / name).read_text()))
    except (OSError, ValueError):
        return None


def row(value: Json) -> Row:
    return value if isinstance(value, dict) else {}


def items(value: Json) -> list[Json]:
    return value if isinstance(value, list) else []


@dataclass
class Objects:
    """One job's objects in the download line: each start, and each landing with its bytes."""

    fetching: list[int] = field(default_factory=list)
    landed: list[tuple[int, int]] = field(default_factory=list)


@dataclass
class Turn:
    what: str = ""
    start: int | None = None
    end: int | None = None
    said: list[str] = field(default_factory=list)


@dataclass
class Span:
    start: int | None = None
    end: int | None = None

    def took(self) -> float:
        return (self.end - self.start) / 1000 if self.start and self.end else 0.0

    def seconds(self) -> dict[str, Json]:
        return {"start": since(self.start), "end": since(self.end)}


@dataclass
class Download(Span):
    bytes: int = 0
    objects: int = 0

    def seconds(self) -> dict[str, Json]:
        return {**super().seconds(), "bytes": self.bytes, "objects": self.objects, "mb_s": round(self.bytes / self.took() / 1e6, 1) if self.took() else None}


objects: dict[Key, Objects] = {}
turns: dict[str, Turn] = {}
# Each emergency (its model, when asked, when here, its rate) and the objects fetched at its order.
emergencies: list[Row] = []
urgent: list[tuple[int, str, int]] = []
# Each stand of a downloader's floor (cl-966): its order, from and to (ms), and what came meanwhile.
floors: list[Row] = []
standing: dict[str, tuple[int, int]] = {}
log = out / "tensord.log"
# The headers and trees resolutions pulled (`pull … header= tree=`, the store's transport log): a job's order since #3115, but
# no job's download.
transport = out / "tensorfs.log"
index = set(re.findall(r"(?:header|tree)=sha256:([0-9a-f]{64})", transport.read_text(errors="replace"))) if transport.exists() else set()
for line in log.read_text(errors="replace").splitlines() if log.exists() else []:
    if found := LOGGED.match(line):
        at, fields = ms(found[1]), dict(re.findall(r"(\w+)=(\S+)", found[3]))
        order = int(fields.get("priority", "0"))
        if order == 0 and at >= ask:
            urgent.append((at, found[2], int(fields.get("bytes", "0"))))
        if at < ask - EARLY or order == 0 or order == (1 << 64) - 1 or fields.get("object") in index:
            continue
        kind, job = CLASSES[min((order - 1) >> 62, 2)], ((order - 1) >> 32) & ((1 << 30) - 1)
        seen = objects.setdefault(("call", 0) if (kind, job) == ("run", 0) else (kind, job), Objects())
        if found[2] == "fetching":
            seen.fetching.append(at)
        else:
            seen.landed.append((at, int(fields.get("bytes", "0"))))
    elif found := FLOOR.match(line):
        fields = dict(re.findall(r"(\w+)=(\S+)", found[2]))
        at, downloader = int(fields.get("at_ms", "0")), fields.get("downloader", "")
        if fields.get("from", "none") != "none" and downloader in standing:
            order, began = standing.pop(downloader)
            if began >= ask:
                floors.append({"order": order, "from_ms": began, "to_ms": at, "bytes": int(fields.get("bytes", "0")),
                               "mb_s": round(float(fields.get("mb_s", "0")), 1)})
        if fields.get("to", "none") != "none":
            standing[downloader] = (int(fields["to"]), at)
    elif found := EMERGENCY.match(line):
        at, fields = ms(found[1]), dict(re.findall(r"(\w+)=(\S+)", found[4]))
        if at < ask:
            continue
        if found[3] == "read now":
            emergencies.append({"model": found[2], "asked_ms": at, "bytes": int(fields.get("bytes", "0"))})
        elif mine := next((e for e in reversed(emergencies) if e["model"] == found[2] and "here_ms" not in e), None):
            mine["here_ms"], mine["mb_s"] = at, round(float(fields.get("mb_s", "0")), 1)
    elif found := TURNS.match(line):
        at, said, run = ms(found[1]), found[2], found[3]
        if at < ask - EARLY:
            continue
        what = re.search(r'what="?([^"]+)"?', found[4])
        turn = turns.setdefault(run, Turn())
        turn.what = turn.what or (what[1] if what else "")
        if said == "its turn":
            turn.start = turn.start or at
        elif said == "done":
            turn.end = at
        else:
            turn.said.append(said)


# A project's emergencies, when TensorD's log has none: the downloader's floor at 0 (its model unnamed, the project's own).
if not emergencies:
    emergencies = [{"model": "", "asked_ms": f["from_ms"], "here_ms": f["to_ms"], "bytes": f["bytes"], "mb_s": f["mb_s"]} for f in floors if f["order"] == 0]


def download(keys: list[Key]) -> Download:
    fetching = [at for k in keys if k in objects for at in objects[k].fetching]
    landed = [seen for k in keys if k in objects for seen in objects[k].landed]
    return Download(min(fetching, default=None), max((at for at, _ in landed), default=None), sum(size for _, size in landed), len(landed))


# A project's install turns, from the board's stages (every 5 s) when TensorD's log has none: each job's or fetch ahead's
# `installing <org>/<package> <release> on this machine (…)`, from the first board that says it to the first that doesn't.
if not turns:
    boards = [cast(Row, json.loads(b)) for b in (out / "board.ndjson").read_text().splitlines() if b.strip()] if (out / "board.ndjson").exists() else []
    for b in boards:
        at, board = num(b.get("at_ms")) or 0, row(b.get("board"))
        now = {f"{kind}:{text(row(e).get('asset')) or text(row(e).get('function'))}": INSTALLING.search(text(row(e).get("stage")))
               for kind in ("jobs", "ahead") for e in items(board.get(kind))}
        for key, said in now.items():
            if said:
                turn = turns.setdefault(f"{key}:{said[1]}", Turn(what=said[1]))
                turn.start = turn.start or at
                turn.end = None
        for key, turn in turns.items():
            if turn.end is None and not now.get(key.rsplit(":", 1)[0]):
                turn.end = at


def install(package: str) -> tuple[Span, list[str]]:
    held = [t for t in turns.values() if re.search(rf"(^|/){re.escape(package)}\b", t.what)]
    starts, ends = [t.start for t in held if t.start], [t.end for t in held if t.end]
    return Span(min(starts, default=None), max(ends, default=None)), [w for t in held for w in t.said]


runs = [row(r) for r in items(row(load("runs.json")).get("runs")) if (num(row(r).get("created_at_ms")) or 0) >= ask - EARLY]
full = {num(r.get("number")): row(load(f"run-{num(r.get('number'))}.json")) or r for r in runs}


def package(target: Json) -> str:
    """A run's package by its target: `_/qwen-image-2/image@0.9.2`, `ahead _/minimax-h3/generate@0.11.1` (an org, then the package
    and its function, at a release), or `qwen-image-2/image`."""
    parts = (text(target).split() or [""])[-1].split("@")[0].split("/")
    return parts[-2] if len(parts) >= 2 else parts[0]


def stages(record: Row) -> dict[str, Json]:
    """A run's own stage times, s (its record's `report.measured`)."""
    measured = [row(stage) for m in items(row(record.get("report")).get("measured")) for stage in items(row(m).get("stages"))]
    return {text(stage.get("name")): round(float(cast(float, stage.get("ms") or 0)) / 1000, 1) for stage in measured}


def one(kind: str, tool: str) -> Row:
    return next((full[num(r.get("number"))] for r in runs if r.get("kind") == kind and package(r.get("target")) == tool), {})


def gpu(record: Row) -> Span:
    start, run_ms = num(record.get("started_at_ms")), num(record.get("execution_ms"))
    return Span(start, start + run_ms if start and run_ms is not None else num(record.get("settled_at_ms")))


picture, video, ahead = one("call", picture_tool), one("call", video_tool), one("warm", video_tool)
own = sorted(job for (kind, job), seen in objects.items() if kind == "run" and seen.landed)
calls = [row(c) for c in items(video.get("calls"))]
first_call = min((t for t in (num(c.get("called_at_ms")) for c in calls) if t is not None), default=None)
board = [cast(Row, json.loads(b)) for b in (out / "board.ndjson").read_text().splitlines() if b.strip()] if (out / "board.ndjson").exists() else []
ahead_seen = next((num(b.get("at_ms")) for b in board if re.search(r'"ahead":\s*\[\s*\{', json.dumps(b.get("board")))), None)

p_install, p_said = install(picture_tool)
p_download, p_gpu = download([("run", own[0])] if own else []), gpu(picture)
a_install, a_said = install(video_tool)
a_download = download([k for k in objects if k[0] == "ahead"])
a_guesses = download([k for k in objects if k[0] == "guess"])
v_sent, v_started, v_settled = num(video.get("created_at_ms")), num(video.get("started_at_ms")), num(video.get("settled_at_ms"))
p_settled = num(picture.get("settled_at_ms"))
picture_chain = p_install.took() + p_download.took() + p_gpu.took()
video_prep = a_install.took() + a_download.took()
video_run = (v_settled - v_started) / 1000 if v_settled and v_started else 0.0
ahead_end = max((t for t in (a_download.end, a_guesses.end) if t), default=None)
ahead_took = Span(a_download.start, ahead_end).took()
waited = round((first_call - v_sent) / 1000, 1) if first_call and v_sent else None
last = max((t for t in (p_settled, v_settled) if t), default=None)
timeline: dict[str, Json] = {
    "ask_unix_ms": ask,
    "seconds_from_ask": {
        "picture": {"run": num(picture.get("number")), "target": text(picture.get("target")), "sent": since(num(picture.get("created_at_ms"))),
                    "install": p_install.seconds(), "said": cast(Json, p_said), "download": p_download.seconds(), "gpu": p_gpu.seconds(),
                    "settled": since(p_settled)},
        "video_ahead": {"run": num(ahead.get("number")), "sent": since(num(ahead.get("created_at_ms"))), "accepted": since(num(ahead.get("accepted_at_ms"))),
                        "seen_on_board": since(ahead_seen), "install": a_install.seconds(), "said": cast(Json, a_said), "download": a_download.seconds(),
                        "guesses": a_guesses.seconds(), "settled": since(num(ahead.get("settled_at_ms"))), "status": text(ahead.get("status"))},
        "video": {"run": num(video.get("number")), "target": text(video.get("target")), "sent": since(v_sent), "accepted": since(num(video.get("accepted_at_ms"))),
                  "started": since(v_started), "first_call": since(first_call), "own_download": download([("run", j) for j in own[1:]]).seconds(),
                  "calls_download": download([("call", 0)]).seconds(),
                  "calls": [{"function": text(c.get("function")), "status": text(c.get("status")), "called": since(num(c.get("called_at_ms"))),
                             "finished": since(num(c.get("finished_at_ms")))} for c in calls],
                  "settled": since(v_settled), "status": text(video.get("status"))},
    },
    "seconds": {
        "end_to_end": since(last),
        "ideal": round(max(picture_chain, video_prep) + video_run, 1),
        "picture_chain": round(picture_chain, 1), "video_install_and_download": round(video_prep, 1), "video_run": round(video_run, 1),
        "video_sent_to_first_call": waited, "video_ahead_download": round(ahead_took, 1),
    },
    "install_turns": {run: {"what": t.what, "start": since(t.start), "end": since(t.end), "said": cast(Json, t.said)} for run, t in turns.items()},
    "picture_stages": cast(Json, stages(picture)),
    # Each stand of a downloader's floor: at 0 an emergency, at a job's order its install's build, while the rest waited.
    "floors": [{"order": cast(int, f["order"]), "class": "emergency" if f["order"] == 0 else "install", "from": since(num(f["from_ms"])),
                "to": since(num(f["to_ms"])), "seconds": round((cast(int, f["to_ms"]) - cast(int, f["from_ms"])) / 1000, 1), "bytes": f["bytes"],
                "mb_s": f["mb_s"], "others_started_during": sum(1 for seen in objects.values() for at in seen.fetching
                                                                 if cast(int, f["from_ms"]) < at < cast(int, f["to_ms"]))} for f in floors],
    # Each emergency: while it came, how many other objects started (none: the link was its) and whether the rest went on after.
    "emergencies": [{"model": e["model"], "asked": since(num(e["asked_ms"])), "here": since(num(e.get("here_ms"))), "bytes": e["bytes"],
                     "mb_s": e.get("mb_s"), "others_started_during": sum(1 for seen in objects.values() for at in seen.fetching
                                                                          if cast(int, e["asked_ms"]) < at < cast(int, e.get("here_ms") or 0)),
                     "others_after": any(at > cast(int, e.get("here_ms") or 1 << 62) for seen in objects.values() for at in seen.fetching)}
                    for e in emergencies],
    "emergency_objects": len([u for u in urgent if u[1] == "landed"]),
    "guesses_by_job": {str(job): download([(kind, job)]).seconds() for kind, job in objects if kind == "guess"},
}
failed: list[str] = []


def check(ok: bool, why: str) -> None:
    if not ok:
        failed.append(why)


check(a_download.bytes > 0, "the video's fetch ahead downloaded nothing")
if a_download.start and p_gpu.end:
    check(a_download.start < p_gpu.end, "the video's weights didn't start coming while the picture rendered")
starts = objects[("run", own[0])].fetching if own else []
if starts:
    # The video's: its fetch ahead's and guesses, never the picture's own on-demand guesses (its job's, cl-966).
    between = [at for k, seen in objects.items() if k[0] in ("ahead", "guess") and k[1] != own[0] for at in seen.fetching if min(starts) + 2000 < at < max(starts)]
    check(not between, f"{len(between)} of the video's objects started while the picture's were starting: the download line is in job order")
if waited is not None and ahead_took:
    check(waited < ahead_took / 2, "the video's run waited about as long as its download takes: nothing was saved")
print(json.dumps(timeline, indent=1))
for why in failed:
    print(why, file=sys.stderr)
sys.exit(1 if failed else 0)
