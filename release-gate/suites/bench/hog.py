"""hog.py <bytes> <lock>: hold <bytes> of touched anonymous memory until killed (pod.sh), mlocked when <lock> is 1. It is the
OOM killer's first pick, so a pod short of memory loses the hog (which `pod.sh check` reports) rather than TensorD. Writes
'<pid> <locked|unlocked|nolock>' to /var/tmp/bench-hog.ready once every page is touched: a refused lock leaves it unlocked,
paged out under pressure, which `check` also sees."""
import ctypes
import mmap
import os
import sys
import time


def main(n: int, lock: bool) -> None:
    try:
        with open('/proc/self/oom_score_adj', 'w') as f:
            f.write('1000')
    except OSError:
        pass
    m = mmap.mmap(-1, n)
    chunk = b'\1' * (64 << 20)
    for at in range(0, n, len(chunk)):
        m.write(chunk[:n - at])
    state = 'nolock'
    if lock:
        state = 'locked' if int(ctypes.CDLL(None).mlockall(1)) == 0 else 'unlocked'
    with open('/var/tmp/bench-hog.ready.tmp', 'w') as f:
        f.write(f'{os.getpid()} {state}')
    os.replace('/var/tmp/bench-hog.ready.tmp', '/var/tmp/bench-hog.ready')
    while True:
        time.sleep(3600)


if __name__ == '__main__':
    main(int(sys.argv[1]), sys.argv[2] == '1')
