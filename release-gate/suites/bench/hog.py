"""hog.py <bytes> <lock>: hold up to <bytes> of touched anonymous memory until killed (pod.sh), mlocked when <lock> is 1. It
grows 2 GiB at a time: a step that stalls (over 20 s) or raises memory pressure is a limit the pod hides, and it backs off a
step. It is the OOM killer's first pick, so a pod short of memory loses the hog (which `pod.sh check` reports) rather than
TensorD. Writes '<pid> <locked|unlocked|nolock> <held bytes> [why short]' to /var/tmp/bench-hog.ready once it stands: a
refused lock leaves it unlocked, paged out under pressure, which `check` also sees."""
import ctypes
import mmap
import os
import sys
import time

STEP = 2 << 30


def pressure() -> float:
    """The share of the last 10 s some task waited on memory; 0 where the kernel does not say."""
    try:
        with open('/proc/pressure/memory') as f:
            return float(f.readline().split()[1].split('=')[1])
    except (OSError, IndexError, ValueError):
        return 0.0


def main(n: int, lock: bool) -> None:
    try:
        with open('/proc/self/oom_score_adj', 'w') as f:
            f.write('1000')
    except OSError:
        pass
    held: list[mmap.mmap] = []
    chunk = b'\1' * (64 << 20)
    short = ''
    while sum(len(m) for m in held) < n:
        start = time.monotonic()
        m = mmap.mmap(-1, min(STEP, n - sum(len(m) for m in held)))
        for at in range(0, len(m), len(chunk)):
            m.write(chunk[:len(m) - at])
        held.append(m)
        if time.monotonic() - start > 20 or pressure() > 10:
            short = 'a 2 GiB step stalled or raised memory pressure'
            for m in held[-2:]:
                m.close()
            del held[-2:]
            break
    state = 'nolock'
    if lock:
        state = 'locked' if int(ctypes.CDLL(None).mlockall(1)) == 0 else 'unlocked'
    with open('/var/tmp/bench-hog.ready.tmp', 'w') as f:
        f.write(f'{os.getpid()} {state} {sum(len(m) for m in held)} {short}'.rstrip())
    os.replace('/var/tmp/bench-hog.ready.tmp', '/var/tmp/bench-hog.ready')
    while True:
        time.sleep(3600)


if __name__ == '__main__':
    main(int(sys.argv[1]), sys.argv[2] == '1')
