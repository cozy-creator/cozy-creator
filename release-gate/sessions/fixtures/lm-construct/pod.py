"""lm-construct's pod side (sessions/lm-construct.sh), run as root over `cozy machine ssh`:
  evict              the store's weights out of the page cache (cold-start.sh's)
  ceiling            TensorFS's own ceilings, from the newest executor environment: disk GB/s and pinned H2D GB/s to GPU 0
  watch <label> <s>  until /tmp/lm-<label>.stop or <s> seconds: every 0.5 s the disk's reads and each executor's I/O
                     (/tmp/lm/<label>.jsonl), and each executor process that starts sampled by py-spy (/tmp/py-spy, if
                     there) at 25 Hz with its idle frames (/tmp/lm/<label>-<pid>.json, speedscope)
  end                every executor process ends (SIGTERM): the next run starts a new one
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
    out = {}
    for d in glob.glob("/proc/[0-9]*"):
        try:
            cmd = open(d + "/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        if "cozy_runtime.internal.executor" in cmd:
            out[int(d[6:])] = d
    return out


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
    ended = sorted(executors())
    for pid in ended:
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    time.sleep(5)
    print(json.dumps({"ended": ended, "left": sorted(set(executors()) & set(ended))}))
elif mode == "watch":
    label, until = sys.argv[2], time.time() + float(sys.argv[3])
    stop = f"/tmp/lm-{label}.stop"
    os.makedirs("/tmp/lm", exist_ok=True)
    seen, spies = set(executors()), {}
    spy = "/tmp/py-spy" if os.access("/tmp/py-spy", os.X_OK) else None
    out = open(f"/tmp/lm/{label}.jsonl", "a")
    while not os.path.exists(stop) and time.time() < until:
        row = {"t": round(time.time(), 3), "disk": {}, "procs": []}
        for line in open("/proc/diskstats"):
            f = line.split()
            if not f[2].startswith(("loop", "ram")) and int(f[3]) + int(f[7]):
                row["disk"][f[2]] = int(f[5]) * 512
        for pid, d in executors().items():
            try:
                io = dict(line.split(": ") for line in open(d + "/io").read().splitlines())
                row["procs"].append([pid, int(io["read_bytes"]), int(io["rchar"])])
            except (OSError, KeyError, ValueError):
                pass
            if pid not in seen:
                seen.add(pid)
                if spy:
                    spies[pid] = subprocess.Popen(
                        [spy, "record", "--pid", str(pid), "--rate", "25", "--idle", "--threads", "--nonblocking",
                         "--format", "speedscope", "-o", f"/tmp/lm/{label}-{pid}.json"],
                        stdout=subprocess.DEVNULL, stderr=open(f"/tmp/lm/{label}-{pid}.err", "w"),
                    )
        out.write(json.dumps(row) + "\n")
        out.flush()
        time.sleep(0.5)
    for p in spies.values():
        if p.poll() is None:
            p.send_signal(signal.SIGINT)
    for p in spies.values():
        try:
            p.wait(120)
        except subprocess.TimeoutExpired:
            p.kill()
    print(json.dumps({"label": label, "spied": sorted(spies)}))
