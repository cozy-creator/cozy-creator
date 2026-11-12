"""cl-905: jobs queued together on one machine, as its three lines served them: each job's install, download and run, from
its record, its events and TensorD's log (TensorFS's `fetching` and `landed`, each with its order). Prints the timeline as
JSON, end to end against the ideal for those spans (the jobs in order through install, download and run, each line one
job at a time); each check that failed on stderr, and exit 1 when one did.

    lines.py <out> <began unix ms> <tensord log> <name>...   (<out>/lines-<name>.*: each run's record and events, in the
                                                             order the machine took them)
"""

import json
import re
import sys
from datetime import datetime
from pathlib import Path

out, began, log, names = Path(sys.argv[1]), int(sys.argv[2]), Path(sys.argv[3]), sys.argv[4:]
MiB = 1 << 20
# A build's numbered steps (`build_on`), and this machine's PyTorch the first build installs: an install's own work, after
# its turn in the install line.
INSTALL = re.compile(r"(installing|updating) .* \d+/\d+$|installing this machine's PyTorch")
LOGGED = re.compile(r"^(\S+)\s+INFO\s+\S+: (fetching|landed) (.*)$")


def events(name):
    path = out / f"lines-{name}.ndjson"
    for line in path.read_text(errors="replace").splitlines() if path.exists() else []:
        if line.startswith("{"):
            try:
                yield json.loads(line)
            except ValueError:
                pass


def record(name):
    try:
        return json.loads((out / f"lines-{name}.json").read_text())
    except (OSError, ValueError):
        return {}


def install(name):
    """Its install: from its first install step to the first stage after its last; and what it said while it waited."""
    stages = sorted((p["at_ms"], p.get("stage") or "") for e in events(name) if isinstance(p := e.get("progress"), dict) and p.get("at_ms"))
    marks = [i for i, (_, stage) in enumerate(stages) if INSTALL.search(stage)]
    said = sorted({w for e in events(name) for w in [(e.get("run") or {}).get("waiting"), e.get("reason")] if isinstance(w, str) and "line to install" in w})
    if not marks:
        return {"start_ms": None, "end_ms": None, "said": said}
    after = next((at for at, stage in stages[marks[-1] + 1 :] if not INSTALL.search(stage)), stages[marks[-1]][0])
    return {"start_ms": stages[marks[0]][0], "end_ms": after, "said": said}


def downloads():
    """Each run's objects from TensorD's log since `began`, by its job (its acceptance, from the order): when each started
    and landed, with its bytes. Only runs' own orders (`priority::next`), not a reader's or a call's."""
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


def took(span):
    return (span["end_ms"] - span["start_ms"]) / 1000 if span.get("start_ms") and span.get("end_ms") else 0.0


failed = []
check = lambda ok, why: ok or failed.append(why)
jobs = downloads()
fetched = sorted(job for job, seen in jobs.items() if seen["landed"])
spans = {}
for name in names:
    run = record(name)
    # A warm run (a download alone) runs nothing on the device: its time is its download's.
    started, settled = None if run.get("kind") == "warm" else run.get("started_at_ms"), run.get("settled_at_ms")
    end = started + run["execution_ms"] if started and isinstance(run.get("execution_ms"), int) else (settled if started else None)
    spans[name] = {
        "run_number": run.get("number"), "kind": run.get("kind"), "target": run.get("target"), "status": run.get("status"),
        "sent_ms": run.get("created_at_ms"), "accepted_ms": run.get("accepted_at_ms"), "install": install(name),
        "download": {"start_ms": None, "end_ms": None, "bytes": 0, "objects": 0}, "run": {"start_ms": started, "end_ms": end},
        "settled_ms": settled,
    }
# The jobs with weights come first, in the order the machine took them: each is the next job in TensorD's log.
for name, job in zip(names, fetched):
    seen = jobs[job]
    spans[name]["machine_job"] = job
    spans[name]["download"] = {
        "start_ms": min(at for at, *_ in seen["fetching"]) if seen["fetching"] else None,
        "end_ms": max(at for at, *_ in seen["landed"]),
        "bytes": sum(size for *_, size in seen["landed"]),
        "objects": len(seen["landed"]),
    }
stages = {name: [took(spans[name][line]) for line in ("install", "download", "run")] for name in names}
# The jobs in order through three lines, each one job at a time: when each finishes each stage at the soonest.
done = [0.0, 0.0, 0.0]
for name in names:
    for k in range(3):
        done[k] = max(done[k], done[k - 1] if k else 0.0) + stages[name][k]
sent = [spans[n]["sent_ms"] for n in names if spans[n]["sent_ms"]]
settled = [spans[n]["settled_ms"] for n in names if spans[n]["settled_ms"]]
timeline = {
    "began_unix_ms": began,
    "jobs": spans,
    "seconds": {
        "stages": {name: dict(zip(("install", "download", "run"), stages[name])) for name in names},
        "end_to_end": (max(settled) - min(sent)) / 1000 if sent and len(settled) == len(names) else None,
        "ideal": done[2],
        "longest_single_chain": max(sum(values) for values in stages.values()),
    },
}
a, b, c = (spans.get(name) for name in ("a", "b", "c"))
# The download line: A's objects all started before B's first, and B took the link only as A's last bytes came.
if a and b:
    seen_a, seen_b = jobs.get(a.get("machine_job"), {}), jobs.get(b.get("machine_job"), {})
    a_last_start = max((at for at, *_ in seen_a.get("fetching", [])), default=None)
    a_last, b_first = a["download"]["end_ms"], b["download"]["start_ms"]
    early = sum(size for at, _, size in seen_b.get("landed", []) if a_last and at < a_last)
    timeline["download_line"] = {
        "a_last_object_started_ms": a_last_start, "b_first_object_started_ms": b_first,
        "a_last_landed_after_b_began_s": (a_last - b_first) / 1000 if a_last and b_first else None,
        "b_bytes_landed_before_a_finished": early,
    }
    check(len(fetched) == 2, f"A's and B's downloads expected in TensorD's log, found {len(fetched)} jobs: {fetched}")
    check(a["download"]["bytes"] > 0 and b["download"]["bytes"] > 0, "A or B fetched nothing: no downloads to order")
    check(a_last_start is not None and b_first is not None and a_last_start <= b_first, f"a B object started ({b_first}) before A's last did ({a_last_start})")
    check(early <= 2 * MiB, f"{early} bytes of B's landed before A's last object did: they split the link")
    if a["run"]["end_ms"] and b["download"]["end_ms"]:
        check(b["download"]["end_ms"] <= a["run"]["end_ms"], "B's weights were not all here before A's run ended: they didn't overlap it")
# The install line: C built after A, and while A downloaded or ran.
if a and c and c["install"]["start_ms"] and a["install"]["end_ms"]:
    check(c["install"]["start_ms"] >= a["install"]["end_ms"] - 1000, "C's environment was built while A's was: the install line is one at a time")
    if a["run"]["end_ms"] and c["install"]["end_ms"]:
        check(c["install"]["end_ms"] <= a["run"]["end_ms"], "C's install didn't end before A's run did: it didn't overlap it")
print(json.dumps(timeline, indent=1))
for why in failed:
    print(why, file=sys.stderr)
sys.exit(1 if failed else 0)
