# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""This container's host memory now: each memory cgroup on this process's path (limit, usage, peak), MemTotal and
MemAvailable, and the processes by RSS. h3-baked-room's check that the baked-page cache stays under the limit."""
import json
from pathlib import Path

import msgspec

from cozy_runtime.author import App

app = App()


class Nothing(msgspec.Struct):
    pass


class Measured(msgspec.Struct):
    measured: str


def read(path: Path) -> int | str | None:
    try:
        text = path.read_text().strip()
    except OSError:
        return None
    return int(text) if text.isdigit() else text


def cgroups() -> list[dict]:
    levels = []
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        hierarchy, controllers, relative = line.split(":", 2)
        v2 = hierarchy == "0"
        if not v2 and "memory" not in controllers.split(","):
            continue
        base = Path("/sys/fs/cgroup") if v2 else Path("/sys/fs/cgroup/memory")
        names = ("memory.max", "memory.high", "memory.current", "memory.peak") if v2 else (
            "memory.limit_in_bytes", "memory.usage_in_bytes", "memory.max_usage_in_bytes")
        at = base / relative.lstrip("/")
        while True:
            row = {"path": str(at)} | {name: read(at / name) for name in names}
            stat = read(at / "memory.stat")
            if isinstance(stat, str):
                keep = ("anon", "file", "shmem", "rss", "cache", "total_rss", "total_cache", "total_shmem")
                row["stat"] = {k: int(v) for k, v in (l.split() for l in stat.splitlines()) if k in keep}
            levels.append(row)
            if at == base or base not in at.parents:
                break
            at = at.parent
    return levels


def processes() -> list[dict]:
    rows = []
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            status = dict(l.split(":", 1) for l in (d / "status").read_text().splitlines() if ":" in l)
            rss = int(status.get("VmRSS", "0 kB").split()[0]) * 1024
            cmd = (d / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")[:160]
        except (OSError, ValueError):
            continue
        rows.append({"pid": int(d.name), "rss": rss, "cmd": cmd})
    return sorted(rows, key=lambda r: -r["rss"])


@app.entrypoint(accelerator=False)
def main(payload: Nothing) -> Measured:
    meminfo = {k: int(v.split()[0]) * 1024 for k, _, v in (l.partition(":") for l in open("/proc/meminfo"))
               if k in ("MemTotal", "MemAvailable", "MemFree", "Cached", "Shmem", "SwapTotal", "SwapFree")}
    procs = processes()
    return Measured(json.dumps({"host_probe": 1, "meminfo": meminfo, "cgroups": cgroups(),
                                "rss_total": sum(p["rss"] for p in procs), "top": procs[:8]}))
