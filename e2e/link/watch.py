"""tfs-300's instruments, each writing one JSON line per sample until <stop> exists (or SIGTERM):

  watch.py kernel <out> <stop> <interval_s> [--disk DEV]   as root: the E2E_LINK chain's bytes per class (iptables, by its
                                                          rules' comments), the bottleneck's qdisc (backlog, drops), TensorD's
                                                          anonymous memory and bytes written, a block device's writes
  watch.py status <out> <stop> <machine>                  `cozy --json machine show <machine>` each second, from the
                                                          project it's run in: its time, and that project's downloads and
                                                          installs (Status activity)
  watch.py probe <out> <stop> <url>                       a GET of <url> each second, 30 s bound: its time or its error
  watch.py stamp <out> <stop> <file>                      each line <file> gains (a run's events), with when it was seen

A slow status read or probe never holds the next one: each tick starts its own, at most 8 at once."""

import json, os, re, signal, subprocess, sys, threading, time, urllib.request

ms = lambda: int(time.time() * 1000)
stopping = threading.Event()
signal.signal(signal.SIGTERM, lambda *_: stopping.set())


def writer(path):
    lock, out = threading.Lock(), open(path, "a")

    def write(row):
        with lock:
            out.write(json.dumps(row) + "\n")
            out.flush()

    return write


def counters():
    text = subprocess.run(["iptables", "-w", "-nvxL", "E2E_LINK"], capture_output=True, text=True).stdout
    found = {}
    for line in text.splitlines()[2:]:
        name = re.search(r"/\* (\S+) \*/", line)
        fields = line.split()
        if name and len(fields) > 1 and fields[1].isdigit():
            found[name[1]] = found.get(name[1], 0) + int(fields[1])
    return found


def qdisc():
    try:
        rows = json.loads(subprocess.run(["tc", "-s", "-j", "qdisc", "show", "dev", "lo"], capture_output=True, text=True).stdout or "[]")
    except ValueError:
        return {}
    for row in rows:
        if row.get("kind") == "tbf":
            return {"backlog": row.get("backlog", 0), "drops": row.get("drops", 0), "sent": row.get("bytes", 0)}
    return {}


def tensord():
    found = {}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            if b"machine\0serve" not in open(f"/proc/{pid}/cmdline", "rb").read():
                continue
            status = open(f"/proc/{pid}/status").read()
            io = dict(line.split(": ") for line in open(f"/proc/{pid}/io").read().splitlines())
            anon = re.search(r"RssAnon:\s+(\d+)", status)
            found = {"pid": int(pid), "anon_kib": int(anon[1]) if anon else None, "wchar": int(io["wchar"]), "rchar": int(io["rchar"])}
        except (OSError, KeyError, ValueError):
            continue
    return found


def disk(dev):
    name = os.path.basename(os.path.realpath(dev))
    for line in open("/proc/diskstats"):
        f = line.split()
        if f[2] == name:
            return {"written": int(f[9]) * 512, "in_flight": int(f[11])}
    return {}


def kernel(out, stop, interval, dev=None):
    write = writer(out)
    while not (stopping.is_set() or os.path.exists(stop)):
        t = ms()
        row = {"t": t, "bytes": counters(), "q": qdisc(), "tensord": tensord()}
        if dev:
            row["disk"] = disk(dev)
        write(row)
        time.sleep(max(0.0, interval - (ms() - t) / 1e3))


def ticking(stop, each):
    slots, n = threading.Semaphore(8), 0
    began = time.time()
    while not (stopping.is_set() or os.path.exists(stop)):
        if slots.acquire(blocking=False):
            threading.Thread(target=lambda: (each(), slots.release()), daemon=True).start()
        n += 1
        time.sleep(max(0.0, began + n - time.time()))


def status(out, stop, machine):
    write = writer(out)

    def each():
        t = ms()
        done = subprocess.run([os.environ.get("COZY_BIN", "cozy"), "--json", "machine", "show", machine], capture_output=True, text=True, timeout=120)
        row = {"t": t, "ms": ms() - t, "ok": done.returncode == 0}
        try:
            shown = json.loads(done.stdout)
            ops = (shown.get("status") or {}).get("activity") or []
            row["ops"] = [{k: op.get(k) for k in ("kind", "subject", "stage", "bytes_done", "bytes_total", "bytes_here", "bytes_per_s", "started_at_ms", "runs")} for op in ops]
            row["state"] = shown.get("state")
        except (ValueError, AttributeError):
            row["err"] = (done.stderr or done.stdout)[-300:]
        write(row)

    ticking(stop, each)


def probe(out, stop, url):
    write = writer(out)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def each():
        t = ms()
        try:
            with opener.open(url, timeout=30) as answer:
                answer.read()
                write({"t": t, "ms": ms() - t, "code": answer.status})
        except Exception as error:  # noqa: BLE001: every failure is the measurement
            write({"t": t, "ms": ms() - t, "error": str(error)[:200]})

    ticking(stop, each)


def stamp(out, stop, path):
    write, at, rest = writer(out), 0, b""
    while True:
        last = stopping.is_set() or os.path.exists(stop)
        try:
            with open(path, "rb") as f:
                f.seek(at)
                got = f.read()
        except OSError:
            got = b""
        at += len(got)
        lines = (rest + got).split(b"\n")
        rest = lines.pop()
        t = ms()
        for line in lines:
            write({"t": t, "line": line.decode(errors="replace")[:2000]})
        if last:
            return
        time.sleep(0.2)


if __name__ == "__main__":
    mode, out, stop, arg = sys.argv[1:5]
    if mode == "kernel":
        kernel(out, stop, float(arg), sys.argv[6] if len(sys.argv) > 6 and sys.argv[5] == "--disk" else None)
    elif mode == "status":
        status(out, stop, arg)
    elif mode == "probe":
        probe(out, stop, arg)
    elif mode == "stamp":
        stamp(out, stop, arg)
    else:
        sys.exit(f"no mode {mode}")
