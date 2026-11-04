"""window.py <first: lock|walk> <project dir> <calcifer pid>: the walk's cell for a Calcifer killed mid-landing. It stops Calcifer the
moment the file it writes first is replaced, and if that is the landing's write with the other not yet written, kills it there. Exits 0
on a kill in the window, 1 if none came. Linux only: inotify."""
import ctypes, json, os, signal, struct, sys, time

first, d, pid = sys.argv[1], sys.argv[2], int(sys.argv[3])
libc = ctypes.CDLL("libc.so.6", use_errno=True)
fd = libc.inotify_init1(0)
IN_MOVED_TO = 0x80
runs = os.path.join(d, ".runs")
os.makedirs(runs, exist_ok=True)
watch = {libc.inotify_add_watch(fd, d.encode(), IN_MOVED_TO): d, libc.inotify_add_watch(fd, runs.encode(), IN_MOVED_TO): runs}

def versions():
    try:
        return len(json.load(open(os.path.join(d, "project.lock")))["assets"]["held"]["versions"])
    except Exception:
        return 0

def owed():
    try:
        return len(json.load(open(os.path.join(runs, "walk.json"))).get("pending", {}))
    except Exception:
        return 0

seen_owed = False
end = time.time() + 240
while time.time() < end:
    buf = os.read(fd, 65536)
    at = 0
    while at < len(buf):
        wd, mask, _, n = struct.unpack_from("iIII", buf, at)
        name = buf[at + 16 : at + 16 + n].rstrip(b"\0").decode()
        at += 16 + n
        target = {"lock": "project.lock", "walk": "walk.json"}[first]
        if name != target:
            continue
        os.kill(pid, signal.SIGSTOP)
        v, o = versions(), owed()
        seen_owed |= o > 0
        hit = (first == "lock" and v >= 1 and o >= 1) or (first == "walk" and seen_owed and o == 0 and v == 0)
        if hit:
            print(f"in the window: {name} replaced, versions={v} owed={o}: killed", flush=True)
            os.kill(pid, signal.SIGKILL)
            sys.exit(0)
        os.kill(pid, signal.SIGCONT)
print("no window came", flush=True)
sys.exit(1)
