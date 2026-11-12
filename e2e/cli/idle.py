"""xs-189: what an idle cozy costs. After a settle, each cozy process's CPU time and wakeups (its threads' context
switches, voluntary and not) over a window, and its threads by name: a line a process, the whole as JSON to <out>. No
Calcifer or TensorD to measure says so on stderr and exits 1.

    idle.py <settle s> <window s> <out>
"""

import json
import os
import sys
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

TICK = os.sysconf("SC_CLK_TCK")
settle, window, out = float(sys.argv[1]), float(sys.argv[2]), Path(sys.argv[3])


@dataclass
class Thread:
    name: str
    first: tuple[int, int]
    last: tuple[int, int]
    born: bool


@dataclass
class Process:
    role: str
    pid: int
    cpu_first: int
    cpu_last: int = 0
    threads: dict[int, Thread] = field(default_factory=dict)


@dataclass
class Named:
    name: str
    threads: int
    wakeups: int
    cpu_ms: int


@dataclass
class Report:
    role: str
    pid: int
    cpu_ms: int
    wakeups: int
    wakeups_per_s: float
    threads: int
    born: int
    by_thread: list[Named]


def read(path: Path) -> str | None:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return None


def role(pid: int) -> str | None:
    raw = read(Path(f"/proc/{pid}/cmdline"))
    if not raw:
        return None
    argv = [a for a in raw.split("\0") if a and a != "--json"]
    if "cozy_runtime.internal.executor" in argv:
        return "executor"
    if not argv or not Path(argv[0]).name.startswith("cozy"):
        return None
    if argv[1:] == ["calcifer"]:
        return "calcifer"
    if argv[1:3] == ["machine", "serve"]:
        return "tensord"
    return f"cozy {' '.join(argv[1:3])}".strip()


def ticks(stat: str) -> tuple[str, int]:
    """A /proc stat line's name and its user + system ticks."""
    name = stat[stat.index("(") + 1 : stat.rindex(")")]
    rest = stat[stat.rindex(")") + 2 :].split()
    return name, int(rest[11]) + int(rest[12])


def switches(status: str) -> int:
    rows = (line.partition(":") for line in status.splitlines())
    return sum(int(value) for key, _, value in rows if key.endswith("ctxt_switches"))


def sample(processes: dict[int, Process], first: bool) -> None:
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        stat = read(entry / "stat")
        if stat is None:
            continue
        if pid not in processes:
            if not first:
                continue
            found = role(pid)
            if found is None:
                continue
            processes[pid] = Process(found, pid, ticks(stat)[1])
        process = processes[pid]
        process.cpu_last = ticks(stat)[1]
        tasks = entry / "task"
        try:
            tids = [int(t.name) for t in tasks.iterdir()]
        except OSError:
            continue
        for tid in tids:
            tstat, tstatus = read(tasks / str(tid) / "stat"), read(tasks / str(tid) / "status")
            if tstat is None or tstatus is None:
                continue
            name, cpu = ticks(tstat)
            now = (switches(tstatus), cpu)
            if tid in process.threads:
                process.threads[tid].last = now
            else:
                process.threads[tid] = Thread(name, now if first else (0, 0), now, not first)


def report(process: Process) -> Report:
    named: dict[str, Named] = {}
    for thread in process.threads.values():
        row = named.setdefault(thread.name, Named(thread.name, 0, 0, 0))
        row.threads += 1
        row.wakeups += thread.last[0] - thread.first[0]
        row.cpu_ms += (thread.last[1] - thread.first[1]) * 1000 // TICK
    wakeups = sum(n.wakeups for n in named.values())
    return Report(
        process.role,
        process.pid,
        (process.cpu_last - process.cpu_first) * 1000 // TICK,
        wakeups,
        round(wakeups / window, 2),
        len(process.threads),
        sum(t.born for t in process.threads.values()),
        sorted(named.values(), key=lambda n: -n.wakeups),
    )


time.sleep(settle)
processes: dict[int, Process] = {}
sample(processes, True)
began = time.monotonic()
while (left := window - (time.monotonic() - began)) > 0:
    time.sleep(min(1.0, left))
    sample(processes, False)
reports = sorted((report(p) for p in processes.values()), key=lambda r: r.role)
out.write_text(json.dumps({"settle_s": settle, "window_s": window, "processes": [asdict(r) for r in reports]}, indent=1))
for r in reports:
    top = ", ".join(f"{n.name} x{n.threads} {n.wakeups}" for n in r.by_thread[:6])
    print(f"{r.role} (pid {r.pid}): {r.cpu_ms} ms CPU, {r.wakeups} wakeups ({r.wakeups_per_s}/s), {r.threads} threads; {top}")
missing = {"calcifer", "tensord"} - {r.role for r in reports}
if missing:
    print(f"no {' or '.join(sorted(missing))} process to measure", file=sys.stderr)
    sys.exit(1)
