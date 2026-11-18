"""On a pod, as root (submit-at-once's `probe`, cl-1202): the pod's TensorD threads each 0.25 s for `seconds`, into `out`.

Each sample is one JSON line: the time, TensorD's pid and each thread as [tid, name, state, wchan, utime+stime ticks,
syscall]. The syscall is readable only when TensorD allows a ptracer (Yama). Once a second, host.txt gets the load and the CPU, I/O
and memory pressure.
"""

import json
import os
import subprocess
import sys
import time

seconds, out = float(sys.argv[1]), sys.argv[2]
os.makedirs(out, exist_ok=True)


def run(*args: str) -> str:
    """A command's output; empty where the pod lacks it."""
    try:
        return subprocess.run(list(args), capture_output=True, text=True, timeout=10).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def read(path: str) -> str:
    try:
        with open(path, "rb") as f:
            return f.read().decode(errors="replace")
    except OSError:
        return ""


def tensord() -> int | None:
    """The service: a `tensord` whose parent is the `tensord` supervising it (the unit's main process)."""
    for pid in filter(str.isdigit, os.listdir("/proc")):
        if read(f"/proc/{pid}/comm").strip() != "tensord":
            continue
        stat = read(f"/proc/{pid}/stat")
        parent = stat[stat.rfind(")") + 2 :].split()[1:2]
        if parent and read(f"/proc/{parent[0]}/comm").strip() == "tensord":
            return int(pid)
    return None


def threads(pid: int) -> list[list[object]]:
    rows: list[list[object]] = []
    try:
        tids = os.listdir(f"/proc/{pid}/task")
    except OSError:
        return rows
    for tid in tids:
        base = f"/proc/{pid}/task/{tid}"
        stat = read(base + "/stat")
        rest = stat[stat.rfind(")") + 2 :].split()
        if len(rest) < 13:
            continue
        rows.append([int(tid), read(base + "/comm").strip(), rest[0], read(base + "/wchan").strip(), int(rest[11]) + int(rest[12]),
                     read(base + "/syscall").strip()[:60]])
    return rows


with open(os.path.join(out, "host.txt"), "w") as host:
    host.write(f"nproc {os.cpu_count()}\nuname {os.uname().release}\n")
    host.write(read("/proc/mounts"))
    host.write(run("df", "-T", "/", "/var/lib/cozy"))
    for block in sorted(os.listdir("/sys/block")):
        host.write(f"{block} {read(f'/sys/block/{block}/queue/scheduler').strip()}\n")
    host.write(f"ptrace_scope {read('/proc/sys/kernel/yama/ptrace_scope').strip()}\n")

began, last_host = time.time(), 0.0
with open(os.path.join(out, "threads.ndjson"), "w") as samples, open(os.path.join(out, "host.txt"), "a") as host:
    while time.time() < began + seconds:
        now, pid = time.time(), tensord()
        if pid is not None:
            rows = threads(pid)
            samples.write(json.dumps({"t": round(now, 3), "pid": pid, "threads": rows}) + "\n")
        if now >= last_host + 1:
            last_host = now
            pressure = " ".join(read(f"/proc/pressure/{kind}").split("\n")[0].replace("some ", f"{kind}:") for kind in ("cpu", "io", "memory"))
            host.write(f"{now:.3f} load {read('/proc/loadavg').strip()} {pressure}\n")
        samples.flush()
        time.sleep(max(0.0, now + 0.25 - time.time()))
