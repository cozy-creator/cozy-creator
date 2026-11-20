"""cold-load's pod side (sessions/cold-load.sh), run as root over `cozy machine ssh`. Each mode prints one JSON line with
the census in it:
  census  the page cache, the store's bytes and how many are resident (mincore), the derived pages' records (their bytes
          are objects in the store), and the host: CPU, cores, RAM and cgroup limit, the store's mount
  drop    the store's files out of the page cache: /proc/sys/vm/drop_caches where the pod allows it, and
          posix_fadvise(DONTNEED) on each file
  forget  the derived pages' records (LoRA bakes), stage memos and learned peaks removed: what a first load on this machine
          finds
  settle  waits, up to 300 s, until the derived pages stop growing: a run's bakes are persisted after it ends
"""

import ctypes
import json
import mmap
import os
import shutil
import sys
import time

TENSORFS = "/var/lib/tensorfs"
DERIVED = f"{TENSORFS}/derived"
GPU = "/var/lib/cozy/tensord/gpu"
PAGE = os.sysconf("SC_PAGE_SIZE")
libc = ctypes.CDLL(None, use_errno=True)
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
FAILED = ctypes.c_void_p(-1).value


def roots() -> list[str]:
    """The store: /var/lib/tensorfs, and any TENSORFS_HOME or COZY_TENSORFS_ROOT a process runs with."""
    found = {TENSORFS}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            env = open(f"/proc/{pid}/environ", "rb").read().split(b"\0")
        except OSError:
            continue
        for kv in env:
            for name in (b"TENSORFS_HOME=", b"COZY_TENSORFS_ROOT="):
                if kv.startswith(name):
                    found.add(kv[len(name):].decode(errors="replace"))
    return sorted(r for r in found if os.path.isdir(r))


def files(top: str, skip: str | None = None) -> list[str]:
    out = []
    for dp, dirs, names in os.walk(top):
        if skip and os.path.abspath(dp).startswith(skip):
            dirs[:] = []
            continue
        for n in names:
            p = os.path.join(dp, n)
            if os.path.isfile(p) and not os.path.islink(p):
                out.append(p)
    return out


def resident(path: str) -> int:
    """Bytes of the file in the page cache."""
    length = size(path)
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return 0
    held = 0
    try:
        for off in range(0, length, 1 << 30):
            n = min(1 << 30, length - off)
            addr = libc.mmap(None, n, mmap.PROT_READ, mmap.MAP_SHARED, fd, off)
            if addr is None or addr == FAILED:
                return 0
            try:
                vec = (ctypes.c_ubyte * ((n + PAGE - 1) // PAGE))()
                if libc.mincore(addr, n, vec) == 0:
                    pages = bytes(vec)
                    held += (len(pages) - pages.count(0)) * PAGE
            finally:
                libc.munmap(addr, n)
    finally:
        os.close(fd)
    return min(held, length)


def size(path: str) -> int:
    try:
        return os.path.getsize(path)
    except OSError:
        return 0


def gb(n: float) -> float:
    return round(n / 1e9, 2)


def store_files() -> list[str]:
    return sorted({f for r in roots() for f in files(r, skip=DERIVED)})


def meminfo() -> dict[str, int]:
    out = {}
    for line in open("/proc/meminfo"):
        k, v = line.split(":", 1)
        out[k] = int(v.split()[0]) * 1024
    return out


def mount(path: str) -> str:
    best, said = "", ""
    for line in open("/proc/self/mountinfo"):
        f = line.split()
        point, rest = f[4], f[f.index("-") + 1:]
        if path.startswith(point) and len(point) > len(best):
            best, said = point, f"{rest[1]} {rest[0]} at {point}"
    return said


def census() -> dict[str, object]:
    store = store_files()
    derived = files(DERIVED) if os.path.isdir(DERIVED) else []
    m = meminfo()
    cpu = next((line.split(":", 1)[1].strip() for line in open("/proc/cpuinfo") if line.startswith("model name")), "")
    limit = ""
    for p in ("/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory/memory.limit_in_bytes"):
        if os.path.exists(p):
            limit = open(p).read().strip()
            break
    return {
        "store_gb": gb(sum(size(f) for f in store)),
        "store_resident_gb": gb(sum(resident(f) for f in store)),
        "derived_records": len(derived),
        "cached_gb": gb(m.get("Cached", 0)),
        "mem_gb": gb(m.get("MemTotal", 0)),
        "available_gb": gb(m.get("MemAvailable", 0)),
        "cgroup_limit": limit,
        "cpu": cpu,
        "cores": len(os.sched_getaffinity(0)),
        "store_mount": mount(TENSORFS),
        "at_ms": int(time.time() * 1000),
    }


mode = sys.argv[1]
said: dict[str, object] = {"mode": mode}
if mode == "drop":
    os.sync()
    try:
        with open("/proc/sys/vm/drop_caches", "w") as out:
            out.write("1\n")
        said["drop_caches"] = "done"
    except OSError as err:
        said["drop_caches"] = f"refused: {err.strerror}"
    n = 0
    for f in store_files() + (files(DERIVED) if os.path.isdir(DERIVED) else []):
        try:
            fd = os.open(f, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
            n += 1
        finally:
            os.close(fd)
    said["fadvised"] = n
elif mode == "forget":
    gone = []
    memos = [f"{GPU}/stage-memo"] + [dp for dp, _, _ in os.walk("/var/lib/cozy") if os.path.basename(dp) == "stage-memo"
                                     and "/generations/" not in dp and "/editable/" not in dp]
    for d in [DERIVED, f"{GPU}/peaks"] + memos:
        if os.path.isdir(d):
            for name in os.listdir(d):
                p = os.path.join(d, name)
                shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) and not os.path.islink(p) else os.remove(p)
            gone.append(d)
    said["cleared"] = sorted(set(gone))
elif mode == "settle":
    began, last, still = time.time(), -1, 0
    while time.time() - began < 300 and still < 3:
        now = sum(size(f) for f in files(DERIVED)) if os.path.isdir(DERIVED) else 0
        still = still + 1 if now == last else 0
        last = now
        time.sleep(3)
    said["settled_s"] = round(time.time() - began, 1)
said.update(census())
print(json.dumps(said))
