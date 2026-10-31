"""A host-RAM squeeze from outside cozy's process tree (cl-697), started by a root shell over `cozy machine ssh`.

squeeze.py <floor MB> <timeout s> <memory.log>: maps populated anonymous memory (1 GB steps, 64 MB near the floor) until
the room TensorD counts (the host's MemAvailable and, for each limited cgroup on this process's path, its limit less
what it uses and cannot give back) is under the floor, and holds it. It prints the memory log's first `end_idle` line written after it began,
the moment it appears, and exits, so its memory goes back at once; `timeout <room MB>` when none came. The OOM killer
takes it first.
"""
import mmap
import os
import sys
import time

BIG, STEP = 1 << 30, 64 << 20
POPULATE = getattr(mmap, "MAP_POPULATE", 0x8000)


def number(path):
    try:
        text = open(path).read().strip()
    except OSError:
        return None
    return None if text == "max" else int(text)


def room():
    info = dict(line.split(":", 1) for line in open("/proc/meminfo"))
    best = int(info["MemAvailable"].split()[0]) * 1024
    relative = next((line[3:].strip() for line in open("/proc/self/cgroup") if line.startswith("0::")), None)
    group = os.path.join("/sys/fs/cgroup", relative.lstrip("/")) if relative is not None else ""
    while group.startswith("/sys/fs/cgroup"):
        limits = [n for n in (number(f"{group}/memory.high"), number(f"{group}/memory.max")) if n is not None]
        used = number(f"{group}/memory.current")
        if limits and used is not None:
            stat = dict(line.split() for line in open(f"{group}/memory.stat"))
            best = min(best, min(limits) - (used - int(stat.get("inactive_file", 0))))
        if group == "/sys/fs/cgroup":
            break
        group = os.path.dirname(group)
    return best


def main():
    floor, timeout, log = int(sys.argv[1]) << 20, float(sys.argv[2]), sys.argv[3]
    with open("/proc/self/oom_score_adj", "w") as adj:
        adj.write("1000")
    follow = open(log)
    follow.seek(0, os.SEEK_END)
    held, deadline = 0, time.monotonic() + timeout
    blocks = []
    print(f"room {room() >> 20} MB at start", file=sys.stderr, flush=True)
    while time.monotonic() < deadline:
        for line in follow.readlines():
            if '"event":"end_idle"' in line:
                print(line.strip(), flush=True)
                print(f"held {held >> 20} MB, room {room() >> 20} MB", file=sys.stderr, flush=True)
                os._exit(0)
        free = room() - floor
        if free > 0:
            size = BIG if free > 2 * BIG else STEP
            blocks.append(mmap.mmap(-1, size, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS | POPULATE))
            held += size
        else:
            time.sleep(0.01)
    print(f"timeout {room() >> 20}", flush=True)
    print(f"held {held >> 20} MB", file=sys.stderr, flush=True)


main()
