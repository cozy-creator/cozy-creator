"""prepare-ahead's timeline (cl-905): the picture's run, the video's fetch ahead and the video's run, each in the machine's three lines,
seconds from the ask. From the runs' records (runs.json, run-<n>.json), the board every 5 s (board.ndjson) and the pod's TensorD log:
each install turn (`install line: …`, by package), and each object's start and landing (`fetching`, `landed`) with its order, whose
class says whose it is: a run's own (`next`, the picture's the first), a fetch ahead's (`ahead`), a guess (`guess`), or a started run's
call (the video's segment). Prints the timeline as JSON, end to end against the longest single chain; each check that failed on
stderr, and exit 1 when one did.

    prepare-ahead.py <out> <ask unix ms> <picture's package> <video's package>
"""

import json
import re
import sys
from datetime import datetime
from pathlib import Path

out, ask = Path(sys.argv[1]), int(sys.argv[2])
LOGGED = re.compile(r"^(\S+)\s+INFO\s+\S+: (fetching|landed) (.*)$")
TURNS = re.compile(r"^(\S+)\s+INFO\s+\S+: install line: (.*?)\s+run=\"?([^\s\"]+)\"?(.*)$")
CLASSES = ("run", "ahead", "guess")


def ms(stamp):
    return int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)


def s(at):
    return None if at is None else round((at - ask) / 1000, 1)


def load(name, default):
    try:
        return json.loads((out / name).read_text())
    except (OSError, ValueError):
        return default


log = (out / "tensord.log").read_text(errors="replace").splitlines() if (out / "tensord.log").exists() else []
objects, turns = {}, {}
for line in log:
    if found := LOGGED.match(line):
        at, fields = ms(found[1]), dict(re.findall(r"(\w+)=(\S+)", found[3]))
        order = int(fields.get("priority", "0"))
        if at < ask - 60_000 or order == 0 or order == (1 << 64) - 1:
            continue
        kind, job = CLASSES[min(((order - 1) >> 62), 2)], ((order - 1) >> 32) & ((1 << 30) - 1)
        key = ("call", 0) if (kind, job) == ("run", 0) else (kind, job)
        objects.setdefault(key, {"fetching": [], "landed": []})[found[2]].append((at, int(fields.get("bytes", "0"))))
    elif found := TURNS.match(line):
        at, said, run = ms(found[1]), found[2], found[3]
        if at < ask - 60_000:
            continue
        what = (re.search(r'what="?([^"]+)"?', found[4]) or [None, ""])[1]
        turn = turns.setdefault(run, {"what": what, "start": None, "end": None, "said": []})
        turn["what"] = turn["what"] or what
        if said == "its turn":
            turn["start"] = turn["start"] or at
        elif said == "done":
            turn["end"] = at
        else:
            turn["said"].append(said)


def download(*keys):
    fetching = [at for k in keys for at, _ in objects.get(k, {}).get("fetching", [])]
    landed = [(at, size) for k in keys for at, size in objects.get(k, {}).get("landed", [])]
    return {"start": min(fetching, default=None), "end": max((at for at, _ in landed), default=None),
            "bytes": sum(size for _, size in landed), "objects": len(landed)}


def install(package):
    held = [t for t in turns.values() if t["what"].startswith(package)]
    return {"start": min((t["start"] for t in held if t["start"]), default=None), "end": max((t["end"] for t in held if t["end"]), default=None),
            "said": [w for t in held for w in t["said"]]}


runs = [r for r in load("runs.json", {}).get("runs", []) if (r.get("created_at_ms") or 0) >= ask - 60_000]
full = {r["number"]: load(f"run-{r['number']}.json", r) for r in runs}
picture_tool, video_tool = sys.argv[3], sys.argv[4]
pkg = lambda target: (target or "").split()[-1].split("/")[0]
picture = next((full[r["number"]] for r in runs if r.get("kind") == "call" and pkg(r.get("target")) == picture_tool), {})
video = next((full[r["number"]] for r in runs if r.get("kind") == "call" and pkg(r.get("target")) == video_tool), {})
ahead = next((full[r["number"]] for r in runs if r.get("kind") == "warm" and pkg(r.get("target")) == video_tool), {})
own = sorted(job for kind, job in objects if kind == "run" and objects[(kind, job)]["landed"])


def gpu(record):
    start = record.get("started_at_ms")
    end = start + record["execution_ms"] if start and isinstance(record.get("execution_ms"), int) else record.get("settled_at_ms")
    return {"start": start, "end": end}


calls = video.get("calls") or []
first_call = min((c.get("called_at_ms") for c in calls if c.get("called_at_ms")), default=None)
board = [json.loads(line) for line in (out / "board.ndjson").read_text().splitlines()] if (out / "board.ndjson").exists() else []
ahead_seen = next((b["at_ms"] for b in board if re.search(r'"ahead":\s*\[\s*\{', json.dumps(b.get("board")))), None)

lines = {
    "picture": {"run": picture.get("number"), "target": picture.get("target"), "sent": picture.get("created_at_ms"),
                "install": install(picture_tool), "download": download(("run", own[0])) if own else download(), "gpu": gpu(picture),
                "settled": picture.get("settled_at_ms")},
    "video_ahead": {"run": ahead.get("number"), "sent": ahead.get("created_at_ms"), "accepted": ahead.get("accepted_at_ms"),
                    "seen_on_board": ahead_seen, "install": install(video_tool), "download": download(*[k for k in objects if k[0] == "ahead"]),
                    "guesses": download(*[k for k in objects if k[0] == "guess"]),
                    "settled": ahead.get("settled_at_ms"), "status": ahead.get("status")},
    "video": {"run": video.get("number"), "target": video.get("target"), "sent": video.get("created_at_ms"), "accepted": video.get("accepted_at_ms"),
              "started": video.get("started_at_ms"), "first_call": first_call,
              "own_download": download(*[("run", j) for j in own[1:]]), "calls_download": download(("call", 0)),
              "calls": [{k: c.get(k) for k in ("function", "status", "called_at_ms", "finished_at_ms")} for c in calls],
              "settled": video.get("settled_at_ms"), "status": video.get("status")},
}


def seconds(value):
    if isinstance(value, dict):
        return {k: (s(v) if k in ("start", "end", "sent", "accepted", "started", "first_call", "settled", "seen_on_board", "called_at_ms", "finished_at_ms") and isinstance(v, int) else seconds(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [seconds(v) for v in value]
    return value


def took(span):
    return (span["end"] - span["start"]) / 1000 if span.get("start") and span.get("end") else 0.0


p, a, v = lines["picture"], lines["video_ahead"], lines["video"]
picture_chain = took(p["install"]) + took(p["download"]) + took(p["gpu"])
video_prep = took(a["install"]) + took(a["download"])
video_work = (v["settled"] - v["started"]) / 1000 if v["settled"] and v["started"] else 0.0
last = max((t for t in (p["settled"], v["settled"]) if t), default=None)
timeline = {
    "ask_unix_ms": ask,
    "seconds_from_ask": seconds(lines),
    "seconds": {
        "end_to_end": s(last),
        "ideal": round(max(picture_chain, video_prep) + video_work, 1),
        "picture_chain": round(picture_chain, 1), "video_install_and_download": round(video_prep, 1), "video_run": round(video_work, 1),
        "video_sent_to_first_call": round((first_call - v["sent"]) / 1000, 1) if first_call and v["sent"] else None,
        "video_ahead_download": round(took({"start": a["download"]["start"], "end": max(t for t in (a["download"]["end"], a["guesses"]["end"]) if t) if a["download"]["end"] else None}), 1),
    },
    "install_turns": seconds({run: t for run, t in turns.items()}),
}
failed = []
check = lambda ok, why: ok or failed.append(why)
check(a["download"]["bytes"] > 0, "the video's fetch ahead downloaded nothing")
if a["download"]["start"] and p["gpu"]["end"]:
    check(a["download"]["start"] < p["gpu"]["end"], "the video's weights didn't start coming while the picture rendered")
pf = objects.get(("run", own[0]), {}).get("fetching", []) if own else []
if pf:
    between = [at for k in objects if k[0] in ("ahead", "guess") for at, _ in objects[k]["fetching"] if min(pf)[0] + 2000 < at < max(pf)[0]]
    check(not between, f"{len(between)} of the video's objects started while the picture's were starting: the download line is in job order")
said = timeline["seconds"]
if said["video_sent_to_first_call"] is not None and said["video_ahead_download"]:
    check(said["video_sent_to_first_call"] < said["video_ahead_download"] / 2, "the video's run waited about as long as its download takes: nothing was saved")
print(json.dumps(timeline, indent=1))
for why in failed:
    print(why, file=sys.stderr)
sys.exit(1 if failed else 0)
