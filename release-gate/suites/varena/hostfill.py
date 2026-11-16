"""hostfill.py <GiB> <ready file>: hold the host's memory so about <GiB> stay available (the tightest of MemAvailable and
this cgroup's limit less what it cannot reclaim), as a smaller computer's would, until killed. Touched anonymous memory,
the OOM killer's first pick: a host short of memory loses the fill, not TensorD or a run. Writes '<pid> <held> <available>'
to the ready file once it stands (cells.sh `lora`, tfs-299)."""
import mmap
import os
import sys
import time

STEP = 256 << 20


def available() -> int:
    a = next(int(line.split()[1]) * 1024 for line in open('/proc/meminfo') if line.startswith('MemAvailable:'))
    try:
        top = open('/sys/fs/cgroup/memory.max').read().strip()
        if top != 'max':
            stat = dict(line.split() for line in open('/sys/fs/cgroup/memory.stat'))
            used = int(open('/sys/fs/cgroup/memory.current').read()) - int(stat['inactive_file']) - int(stat['active_file'])
            a = min(a, int(top) - used)
    except (OSError, KeyError, ValueError):
        pass
    return a


def main(keep: int, ready: str) -> None:
    with open('/proc/self/oom_score_adj', 'w') as f:
        f.write('1000')
    held: list[mmap.mmap] = []
    ones = b'\1' * (16 << 20)
    while available() > keep + STEP:
        m = mmap.mmap(-1, STEP, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS)
        for _ in range(STEP // len(ones)):
            m.write(ones)
        held.append(m)
    with open(ready, 'w') as f:
        f.write(f'{os.getpid()} {len(held) * STEP} {available()}\n')
    while True:
        time.sleep(3600)


if __name__ == '__main__':
    main(int(float(sys.argv[1]) * 2**30), sys.argv[2])
