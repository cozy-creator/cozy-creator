"""hog.py <bytes> <lock>: hold <bytes> of touched anonymous memory the OOM killer never picks, mlocked when <lock> is 1,
until killed (pod.sh). Writes '<pid> <mlockall result>' to /var/tmp/bench-hog.ready once every page is touched."""
import ctypes
import mmap
import os
import sys
import time


def main(n: int, lock: bool) -> None:
    try:
        with open('/proc/self/oom_score_adj', 'w') as f:
            f.write('-1000')
    except OSError:
        pass  # no CAP_SYS_RESOURCE: the OOM killer may pick the hog, which `pod.sh check` then reports
    m = mmap.mmap(-1, n)
    chunk = b'\1' * (64 << 20)
    for at in range(0, n, len(chunk)):
        m.write(chunk[:n - at])
    locked = int(ctypes.CDLL(None).mlockall(1)) if lock else 0
    if locked:
        sys.exit(f'mlockall refused ({locked}): with swap the hog would page out')
    with open('/var/tmp/bench-hog.ready', 'w') as f:
        f.write(f'{os.getpid()} {locked}')
    while True:
        time.sleep(3600)


if __name__ == '__main__':
    main(int(sys.argv[1]), sys.argv[2] == '1')
