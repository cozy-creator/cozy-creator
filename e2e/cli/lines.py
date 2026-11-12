"""cl-905: two jobs queued together on one machine, as its three lines served them: each job's install, download and run,
from its record, its events and TensorD's log (TensorFS's `fetching` and `landed`, each with its order). Prints the
timeline as JSON, end to end against the ideal for those spans (A then B through install, download and run, each line one
job at a time); each check that failed on stderr, and exit 1 when one did.

    lines.py <out> <began unix ms> <tensord log>   (<out>/lines-a.* and lines-b.*: the runs' records and events)
"""

import json
import re
import sys
from datetime import datetime
from pathlib import Path

out, began, log = Path(sys.argv[1]), int(sys.argv[2]), Path(sys.argv[3])
MiB = 1 << 20
# A build's numbered steps (`build_on`), and this machine's PyTorch the first build installs: an install's own work, after
# its turn in the install line.
INSTALL = re.compile(r"(installing|updating) .* \d+/\d+$|installing this machine's PyTorch")
LOGGED = re.compile(r"^(\S+)\s+INFO\s+\S+: (fetching|landed) (.*)$")


def events(name):
    path = out / f"{name}.ndjson"
    for line in path.read_text(errors="replace").splitlines() if path.exists() else []:
        if line.startswith("{"):
            try:
                yield json.loads(line)
            except ValueError:
                pass


def record(name):
    try:
        return json.loads((out / f"{name}.json").read_text())
    except (OSError, ValueError):
        return {}


def install(name):
    """Its install: from its first install stage to the first stage after its last; and what it said while it waited."""
    stages = sorted((p["at_ms"], p.get("stage") or "") for e in events(name) if isinstance(p := e.get("progress"), dict) and p.get("at_ms"))
    marks = [i for i, (_, stage) in enumerate(stages) if INSTALL.search(stage)]
    said = sorted({w for e in events(name) for w in [(e.get("run") or {}).get("waiting")] if w and "install" in w})
    if not marks:
        return {"start_ms": None, "end_ms": None, "said": said}
    after = next((at for at, stage in stages[marks[-1] + 1 :] if not INSTALL.search(stage)), stages[marks[-1]][0])
    return {"start_ms": stages[marks[0]][0], "end_ms": after, "said": said}


def downloads():
    """Each run's objects from TensorD's log since `began`, by its job (its acceptance, from the order): when each was
    started and when it landed, with its bytes. Only runs' own orders (`priority::next`), not a reader's or a call's."""
    jobs = {}
    for line in log.read_text(errors="replace").splitlines() if log.exists() else []:
        found = LOGGED.match(line)
        if not found:
            continue
        at = int(datetime.fromisoformat(found[1].replace("Z", "+00:00")).timestamp() * 1000)
        fields = dict(re.findall(r"(\w+)=(\S+)", found[3]))
        order = int(fields.get("priority", "0"))
        if at < began or order == 0 or order >= 1 << 62:
            continue
        job = ((order - 1) >> 32) & ((1 << 30) - 1)
        if job:
            jobs.setdefault(job, {"fetching": [], "landed": []})[found[2]].append((at, fields.get("object"), int(fields.get("bytes", "0"))))
    return jobs


def ms(value):
    return value if isinstance(value, int) else None


jobs, failed = downloads(), []
check = lambda ok, why: ok or failed.append(why)
lined = sorted(job for job, seen in jobs.items() if seen["landed"])
timeline = {"began_unix_ms": began, "jobs": {}}
spans = {}
for name, job in zip(("a", "b"), lined[:2] + [None] * (2 - len(lined[:2]))):
    run = record(f"lines-{name}")
    seen = jobs.get(job, {"fetching": [], "landed": []})
    started, settled = ms(run.get("started_at_ms")), ms(run.get("settled_at_ms"))
    end = started + run["execution_ms"] if started and isinstance(run.get("execution_ms"), int) else settled
    spans[name] = {
        "run_number": run.get("number"),
        "target": run.get("target"),
        "status": run.get("status"),
        "machine_job": job,
        "sent_ms": ms(run.get("created_at_ms")),
        "accepted_ms": ms(run.get("accepted_at_ms")),
        "install": install(f"lines-{name}"),
        "download": {
            "start_ms": min((at for at, *_ in seen["fetching"]), default=None),
            "end_ms": max((at for at, *_ in seen["landed"]), default=None),
            "bytes": sum(size for *_, size in seen["landed"]),
            "objects": len(seen["landed"]),
        },
        "run": {"start_ms": started, "end_ms": end},
        "settled_ms": settled,
    }
timeline["jobs"] = spans
a, b = spans.get("a"), spans.get("b")


def took(span):
    return (span["end_ms"] - span["start_ms"]) / 1000 if span and span.get("start_ms") and span.get("end_ms") else 0.0


stages = {name: [took(spans[name]["install"]), took(spans[name]["download"]), took(spans[name]["run"])] for name in "ab"}
# A then B through three lines, each one job at a time: the longest of A's first k stages then B's from k on.
ideal = max(sum(stages["a"][: k + 1]) + sum(stages["b"][k:]) for k in range(3))
first = min((t for t in (a["sent_ms"], b["sent_ms"]) if t), default=None)
last = max(a["settled_ms"], b["settled_ms"]) if a["settled_ms"] and b["settled_ms"] else None
gaps = {}
if b["install"]["start_ms"] and a["install"]["end_ms"]:
    gaps["b_install_after_a_install_s"] = (b["install"]["start_ms"] - a["install"]["end_ms"]) / 1000
if b["download"]["start_ms"] and a["download"]["end_ms"] and b["install"]["end_ms"]:
    gaps["b_download_after_its_turn_s"] = (b["download"]["start_ms"] - max(a["download"]["end_ms"], b["install"]["end_ms"])) / 1000
if b["run"]["start_ms"] and a["run"]["end_ms"] and b["download"]["end_ms"]:
    gaps["b_run_after_its_turn_s"] = (b["run"]["start_ms"] - max(a["run"]["end_ms"], b["download"]["end_ms"])) / 1000
timeline["seconds"] = {
    "stages": {name: dict(zip(("install", "download", "run"), values)) for name, values in stages.items()},
    "end_to_end": (last - first) / 1000 if first and last else None,
    "ideal": ideal,
    "longest_single_chain": max(sum(values) for values in stages.values()),
    "waits_beyond_the_lines": gaps,
}
# The download line: A's objects all started before B's first, and B took the link only as A's last bytes came.
seen_a, seen_b = jobs.get(a["machine_job"], {}), jobs.get(b["machine_job"], {})
a_last_start = max((at for at, *_ in seen_a.get("fetching", [])), default=None)
a_last_landed = a["download"]["end_ms"]
b_first = b["download"]["start_ms"]
early = sum(size for at, _, size in seen_b.get("landed", []) if a_last_landed and at < a_last_landed)
timeline["download_line"] = {
    "a_last_object_started_ms": a_last_start,
    "b_first_object_started_ms": b_first,
    "a_last_landed_after_b_began_s": (a_last_landed - b_first) / 1000 if a_last_landed and b_first else None,
    "b_bytes_landed_before_a_finished": early,
}
check(a["download"]["bytes"] > 0, "A fetched no weights: no download to order")
check(b["download"]["bytes"] > 0, "B fetched no weights: no download to order")
check(a_last_start is not None and b_first is not None and a_last_start <= b_first, f"a B object started ({b_first}) before A's last ({a_last_start})")
check(early <= 2 * MiB, f"{early} bytes of B's landed before A's last object did: they split the link")
if b["install"]["start_ms"] and a["install"]["start_ms"] and a["install"]["end_ms"]:
    check(b["install"]["start_ms"] >= a["install"]["end_ms"] - 1000, "B's environment was built while A's was: the install line is one at a time")
if a["run"]["end_ms"] and b["download"]["end_ms"]:
    check(b["download"]["end_ms"] <= a["run"]["end_ms"], "B's weights were not all here before A's run ended: they didn't overlap it")
check(len(lined) == 2, f"two jobs' downloads expected in TensorD's log, found {len(lined)}: {lined}")
print(json.dumps(timeline, indent=1))
for why in failed:
    print(why, file=sys.stderr)
sys.exit(1 if failed else 0)
