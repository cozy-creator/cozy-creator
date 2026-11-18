"""On a pod, as root (submit-at-once's `probe`, cl-1202): the pod's TensorD threads each 0.25 s for `seconds`, into `out`.

Each sample is one JSON line: the time, TensorD's pid and each thread as [tid, name, state, wchan, utime+stime ticks,
syscall]. The syscall is readable only when TensorD allows a ptracer. With `gdb`, `thread apply all bt` is also written
2, 5 and 9 s after TensorD's first `import-pages` thread appears. Once a second, host.txt gets the load, the CPU and I/O
pressure, and TensorD's API connections (`ss -tni`: queues, RTT, window).
"""

import json
import os
import re
import subprocess
import sys
import time

seconds, out, gdb = float(sys.argv[1]), sys.argv[2], sys.argv[3:] == ["gdb"]
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
    """The service: a `tensord` whose parent is the `tensord` supervising it (restart.sh's `service`)."""
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

began, dumps, record, last_host = time.time(), [2.0, 5.0, 9.0], None, 0.0
with open(os.path.join(out, "threads.ndjson"), "w") as samples, open(os.path.join(out, "host.txt"), "a") as host:
    while time.time() < began + seconds:
        now, pid = time.time(), tensord()
        if pid is not None:
            rows = threads(pid)
            samples.write(json.dumps({"t": round(now, 3), "pid": pid, "threads": rows}) + "\n")
            if record is None and any(row[1] == "import-pages" for row in rows):
                record = now
            while gdb and record is not None and dumps and now >= record + dumps[0]:
                at = dumps.pop(0)
                done = subprocess.run(["timeout", "60", "gdb", "-p", str(pid), "-batch", "-nx", "-ex", "set pagination off",
                                       "-ex", "set print frame-arguments none", "-ex", "thread apply all bt 30"],
                                      capture_output=True, text=True)
                with open(os.path.join(out, f"gdb-{at:g}s.txt"), "w") as dump:
                    dump.write(f"pid {pid} at {now:.3f} took {time.time() - now:.3f}s\n{done.stdout}\n{done.stderr}")
        if now >= last_host + 1:
            last_host = now
            pressure = " ".join(read(f"/proc/pressure/{kind}").split("\n")[0].replace("some ", f"{kind}:") for kind in ("cpu", "io", "memory"))
            host.write(f"{now:.3f} load {read('/proc/loadavg').strip()} {pressure}\n")
            # TensorD's API connections (its listening ports'): their queues, RTT and window, to tell a stalled link from a stalled process.
            listening = run("ss", "-tlnpH")
            ports = {line.split()[3].rsplit(":", 1)[1] for line in listening.splitlines() if '"tensord"' in line and len(line.split()) > 3}
            if ports:
                match = " or ".join(f"sport = :{p}" for p in sorted(ports)).split()
                api = run("ss", "-tniH", "state", "established", "(", *match, ")")
                # Queues, RTT and windows only: no address of the pod or its peers goes into a published artifact.
                api = re.sub(r"\S+:\d+\s+\S+:\d+", "conn", api)
                host.write(f"{now:.3f} api {' '.join(api.split())[:4000]}\n")
        samples.flush()
        time.sleep(max(0.0, now + 0.25 - time.time()))
