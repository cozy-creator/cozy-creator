"""cl-905: jobs queued together on one machine, as its three lines served them: each job's install, download and run, from
its record and TensorD's log (each turn of its install line; TensorFS's `fetching` and `landed`, each with its order). An
install of local code is a warm run of its own, before its call: `lines-runs.json` (`cozy run ls --all`) names whose it is.
Prints the timeline as JSON, end to end against the ideal for those spans (the jobs in order through install, download and
run, each line one job at a time); each check that failed on stderr, and exit 1 when one did.

    lines.py <out> <began unix ms> <tensord log> <name>...   (<out>/lines-<name>.*: each run's record and events, in the
                                                             order the machine took them)
"""

import json
import re
import sys
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path
from typing import TypeAlias, cast

Json: TypeAlias = str | int | float | bool | None | list["Json"] | dict[str, "Json"]
Row: TypeAlias = dict[str, Json]
# An object's start or landing: when, which, its bytes.
Seen: TypeAlias = tuple[int, str | None, int]

out, began, log, names = Path(sys.argv[1]), int(sys.argv[2]), Path(sys.argv[3]), sys.argv[4:]
MiB = 1 << 20
LOGGED = re.compile(r"^(\S+)\s+INFO\s+\S+: (fetching|landed) (.*)$")
TURNS = re.compile(r"^(\S+)\s+INFO\s+\S+: install line: (.*?)\s+run=\"?([^\s\"]+)\"?")


def ms(stamp: str) -> int:
    return int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)


def num(value: Json) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def text(value: Json) -> str | None:
    return value if isinstance(value, str) else None


def load(path: Path) -> Json:
    try:
        return cast(Json, json.loads(path.read_text()))
    except (OSError, ValueError):
        return None


def record(name: str) -> Row:
    found = load(out / f"lines-{name}.json")
    return found if isinstance(found, dict) else {}


def log_lines() -> list[str]:
    return log.read_text(errors="replace").splitlines() if log.exists() else []


@dataclass
class Turn:
    """A build's turn in the install line: when it came and ended, what it said while it waited, and whose run it was."""

    start_ms: int | None = None
    end_ms: int | None = None
    said: list[str] = field(default_factory=list)
    run_number: int | None = None
    target: str | None = None


@dataclass
class Span:
    start_ms: int | None = None
    end_ms: int | None = None

    def took(self) -> float:
        return (self.end_ms - self.start_ms) / 1000 if self.start_ms and self.end_ms else 0.0


@dataclass
class Download(Span):
    bytes: int = 0
    objects: int = 0


@dataclass
class Job:
    run_number: int | None
    kind: str | None
    target: str | None
    status: str | None
    sent_ms: int | None
    accepted_ms: int | None
    install: Turn
    download: Download
    run: Span
    settled_ms: int | None
    machine_job: int | None = None


def turns() -> dict[str, Turn]:
    """Each run's turns in the install line since `began`, from TensorD's log: when its turn came and ended, and what it
    said while it waited."""
    held: dict[str, Turn] = {}
    for line in log_lines():
        found = TURNS.match(line)
        if not found or ms(found[1]) < began:
            continue
        at, said, run = ms(found[1]), found[2], found[3]
        turn = held.setdefault(run, Turn())
        if said == "its turn":
            turn.start_ms = turn.start_ms or at
        elif said == "done":
            turn.end_ms = at
        else:
            turn.said.append(said)
    return held


def installs() -> dict[str, Turn]:
    """Each job's install: its call's own turn in the install line, else its warm install run's (local code), by package."""
    listed = load(out / "lines-runs.json")
    rows = listed.get("runs") if isinstance(listed, dict) else None
    runs = {text(row.get("id")): row for row in (rows if isinstance(rows, list) else []) if isinstance(row, dict)}
    found = turns()
    for run_id, turn in found.items():
        run = runs.get(run_id, {})
        turn.run_number, turn.target = num(run.get("number")), text(run.get("target"))
    return found


def resolved() -> set[str]:
    """The headers and trees resolutions pulled (the store's transport log, `pull … header=sha256:… tree=sha256:…`): a job's
    order since #3115, but its weights' index, never its download."""
    path = out / "lines.tensorfs.log"
    text_ = path.read_text(errors="replace") if path.exists() else ""
    return set(re.findall(r"(?:header|tree)=sha256:([0-9a-f]{64})", text_))


def downloads() -> dict[int, dict[str, list[Seen]]]:
    """Each run's objects from TensorD's log since `began`, by its job (its acceptance, from the order): when each started
    and landed, with its bytes. Only runs' own orders (`priority::next`), not a reader's or a call's, and its weights, not
    the headers and trees its resolution pulled."""
    jobs: dict[int, dict[str, list[Seen]]] = {}
    index = resolved()
    for line in log_lines():
        found = LOGGED.match(line)
        if not found:
            continue
        at = ms(found[1])
        fields = dict(re.findall(r"(\w+)=(\S+)", found[3]))
        order = int(fields.get("priority", "0"))
        if at < began or order == 0 or order >= 1 << 62 or fields.get("object") in index:
            continue
        job = ((order - 1) >> 32) & ((1 << 30) - 1)
        if job:
            jobs.setdefault(job, {"fetching": [], "landed": []})[found[2]].append((at, fields.get("object"), int(fields.get("bytes", "0"))))
    return jobs


failed: list[str] = []


def check(ok: bool, why: str) -> None:
    if not ok:
        failed.append(why)


jobs = downloads()
fetched = sorted(job for job, seen in jobs.items() if seen["landed"])
built = installs()
spans: dict[str, Job] = {}
for name in names:
    run = record(name)
    target = text(run.get("target"))
    package = (target or "").rsplit("/", 1)[0]
    own = [turn for run_id, turn in built.items() if run_id == text(run.get("id")) or (package and (turn.target or "").startswith(package))]
    # A warm run (a download alone) runs nothing on the device: its time is its download's.
    started, settled, execution = None if text(run.get("kind")) == "warm" else num(run.get("started_at_ms")), num(run.get("settled_at_ms")), num(run.get("execution_ms"))
    end = started + execution if started and execution is not None else (settled if started else None)
    spans[name] = Job(
        run_number=num(run.get("number")), kind=text(run.get("kind")), target=target, status=text(run.get("status")),
        sent_ms=num(run.get("created_at_ms")), accepted_ms=num(run.get("accepted_at_ms")),
        install=own[0] if own else Turn(), download=Download(), run=Span(started, end), settled_ms=settled,
    )
# The jobs with weights come first, in the order the machine took them: each is the next job in TensorD's log.
for name, job in zip(names, fetched):
    seen = jobs[job]
    spans[name].machine_job = job
    spans[name].download = Download(
        start_ms=min(at for at, *_ in seen["fetching"]) if seen["fetching"] else None,
        end_ms=max(at for at, *_ in seen["landed"]),
        bytes=sum(size for *_, size in seen["landed"]),
        objects=len(seen["landed"]),
    )
lines = ("install", "download", "run")
stages = {name: [Span(spans[name].install.start_ms, spans[name].install.end_ms).took(), spans[name].download.took(), spans[name].run.took()] for name in names}
# The jobs in order through three lines, each one job at a time: when each finishes each stage at the soonest.
done = [0.0, 0.0, 0.0]
for name in names:
    for k in range(3):
        done[k] = max(done[k], done[k - 1] if k else 0.0) + stages[name][k]
sent = [t for n in names for t in (spans[n].sent_ms, spans[n].install.start_ms) if t]
settled_all = [t for n in names if (t := spans[n].settled_ms)]
timeline: Row = {
    "began_unix_ms": began,
    "jobs": cast(Json, {name: asdict(job) for name, job in spans.items()}),
    "install_line": cast(Json, {run_id: asdict(turn) for run_id, turn in built.items()}),
    "seconds": {
        "stages": {name: dict(zip(lines, stages[name])) for name in names},
        "end_to_end": (max(settled_all) - min(sent)) / 1000 if sent and len(settled_all) == len(names) else None,
        "ideal": done[2],
        "longest_single_chain": max(sum(values) for values in stages.values()),
    },
}
a, b, c = (spans.get(name) for name in ("a", "b", "c"))
# The download line: A's objects all started before B's first, and B took the link only as A's last bytes came.
if a and b:
    empty: dict[str, list[Seen]] = {}
    seen_a = jobs.get(a.machine_job, empty) if a.machine_job is not None else empty
    seen_b = jobs.get(b.machine_job, empty) if b.machine_job is not None else empty
    a_last_start = max((at for at, *_ in seen_a.get("fetching", [])), default=None)
    a_last, b_first = a.download.end_ms, b.download.start_ms
    early = sum(size for at, _, size in seen_b.get("landed", []) if a_last and at < a_last)
    timeline["download_line"] = {
        "a_last_object_started_ms": a_last_start, "b_first_object_started_ms": b_first,
        "a_last_landed_after_b_began_s": (a_last - b_first) / 1000 if a_last and b_first else None,
        "b_bytes_landed_before_a_finished": early,
    }
    check(len(fetched) == 2, f"A's and B's downloads expected in TensorD's log, found {len(fetched)} jobs: {fetched}")
    check(a.download.bytes > 0 and b.download.bytes > 0, "A or B fetched nothing: no downloads to order")
    check(a_last_start is not None and b_first is not None and a_last_start <= b_first, f"a B object started ({b_first}) before A's last did ({a_last_start})")
    check(early <= 2 * MiB, f"{early} bytes of B's landed before A's last object did: they split the link")
    if a.run.end_ms and b.download.end_ms:
        check(b.download.end_ms <= a.run.end_ms, "B's weights were not all here before A's run ended: they didn't overlap it")
# The install line: one build at a time, and one asked while another built waited, saying its place.
held = sorted((t.start_ms, t.end_ms or t.start_ms, run_id) for run_id, t in built.items() if t.start_ms)
check(len(held) >= 2, f"two builds expected in the install line, found {len(held)}: {[asdict(t) for t in built.values()]}")
for (_, end, first), (start, _, then) in zip(held, held[1:]):
    check(start >= end, f"{then}'s build began before {first}'s ended: the install line is one at a time")
check(any(t.said for t in built.values()), "no build waited its turn in the install line: A's and C's installs never met")
if a and c and c.install.end_ms and a.run.end_ms:
    check(c.install.end_ms <= a.run.end_ms, "C's install didn't end before A's run did: it didn't overlap A")
print(json.dumps(timeline, indent=1))
for why in failed:
    print(why, file=sys.stderr)
sys.exit(1 if failed else 0)
