"""A host-RAM squeeze from outside cozy's process tree (cl-697), started by a root shell over `cozy machine ssh`.

squeeze.py <floor MB> <timeout s> <memory.log>: touches anonymous memory in 64 MB steps until the room TensorD counts
(the host's MemAvailable and, for each limited cgroup on this process's path, its limit less what it uses and cannot
give back) is under the floor, and holds it. It prints the memory log's first `end_idle` line written after it began,
the moment it appears, and exits, so its memory goes back at once; `timeout <room MB>` when none came. The OOM killer
takes it first.
"""
import mmap
import os
import sys
import time

STEP = 64 << 20


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
    held, deadline = [], time.monotonic() + timeout
    print(f"room {room() >> 20} MB at start", file=sys.stderr, flush=True)
    while time.monotonic() < deadline:
        for line in follow.readlines():
            if '"event":"end_idle"' in line:
                print(line.strip(), flush=True)
                print(f"held {len(held) * STEP >> 20} MB, room {room() >> 20} MB", file=sys.stderr, flush=True)
                os._exit(0)
        if room() > floor:
            block = mmap.mmap(-1, STEP)
            block.write(b"\1" * STEP)
            held.append(block)
        else:
            time.sleep(0.01)
    print(f"timeout {room() >> 20}", flush=True)
    print(f"held {len(held) * STEP >> 20} MB", file=sys.stderr, flush=True)


main()
