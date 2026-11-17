"""lm-construct's pod side (sessions/lm-construct.sh), run as root over `cozy machine ssh`:
  evict    the store's weights out of the page cache (cold-start.sh's)
  ceiling  TensorFS's own ceilings, from the newest executor environment: disk GB/s and pinned H2D GB/s to GPU 0
  end      every executor process ends (SIGTERM): the next run starts a new one
"""

import glob
import json
import os
import signal
import subprocess
import sys
import time


def roots(name):
    found = set()
    for env in glob.glob("/proc/[0-9]*/environ"):
        try:
            for kv in open(env, "rb").read().split(b"\0"):
                if kv.startswith(name.encode() + b"="):
                    found.add(kv.split(b"=", 1)[1].decode())
        except OSError:
            pass
    return sorted(r for r in found if os.path.isdir(r))


def executors():
    out = []
    for d in glob.glob("/proc/[0-9]*"):
        try:
            cmd = open(d + "/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        if "cozy_runtime.internal.executor" in cmd:
            out.append(int(d[6:]))
    return sorted(out)


mode = sys.argv[1]
if mode == "evict":
    tops = set(roots("TENSORFS_HOME")) | set(roots("COZY_TENSORFS_ROOT"))
    files = sorted({os.path.join(dp, f) for r in tops for dp, _, fs in os.walk(r) for f in fs})
    files = [f for f in files if os.path.isfile(f) and not os.path.islink(f) and os.path.getsize(f) > (1 << 20)]
    os.sync()
    for f in files:
        fd = os.open(f, os.O_RDONLY)
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
    print(json.dumps({"files": len(files), "gb": round(sum(os.path.getsize(f) for f in files) / 1e9, 2)}))
elif mode == "ceiling":
    envs = sorted(glob.glob("/var/lib/cozy/tensord/generations/*/env/bin/python"), key=os.path.getmtime)
    store = (roots("TENSORFS_HOME") or roots("COZY_TENSORFS_ROOT") or [None])[0]
    if not envs:
        print(json.dumps({"error": "no executor environment"}))
        sys.exit()
    code = (
        "import json, sys, tensorfs; c = tensorfs.ceiling(0, sys.argv[1] or None); "
        "print(json.dumps({'disk_gbps': c.disk_gbps, 'disk_direct': c.disk_direct, 'h2d_gbps': c.h2d_gbps, 'streams': c.streams}))"
    )
    done = subprocess.run([envs[-1], "-c", code, store or ""], capture_output=True, text=True, timeout=300)
    print(done.stdout.strip() or json.dumps({"error": done.stderr[-400:]}))
elif mode == "end":
    ended = executors()
    for pid in ended:
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    time.sleep(5)
    print(json.dumps({"ended": ended, "left": sorted(set(executors()) & set(ended))}))
