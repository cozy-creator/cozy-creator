"""On a rented pod, over `cozy machine ssh` (prepare-ahead.sh, install-share.sh): its TensorFS store emptied (each root its processes
name, every file but its format) and its executors ended, so the next run starts cold. No verb empties a rented machine's store:
`cozy model uninstall` is this computer's only."""
import glob, json, os, shutil, signal


def roots(name: str) -> list[str]:
    found: set[str] = set()
    for env in glob.glob("/proc/[0-9]*/environ"):
        try:
            for kv in open(env, "rb").read().split(b"\0"):
                if kv.startswith(name.encode() + b"="):
                    found.add(kv.split(b"=", 1)[1].decode())
        except OSError:
            pass
    return sorted(r for r in found if os.path.isdir(r))


stores = sorted(set(roots("COZY_TENSORFS_ROOT")) | set(roots("TENSORFS_HOME")))  # before the executors that name one end
ended = []
for d in glob.glob("/proc/[0-9]*"):
    try:
        if b"cozy_runtime.internal.executor" in open(d + "/cmdline", "rb").read():
            os.kill(int(d[6:]), signal.SIGKILL); ended.append(int(d[6:]))
    except OSError:
        pass
emptied: dict[str, object] = {}
for root in stores:
    size = sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fs in os.walk(root) for f in fs if os.path.isfile(os.path.join(dp, f)))
    for entry in os.listdir(root):
        path = os.path.join(root, entry)
        if entry == "format":
            continue
        shutil.rmtree(path, ignore_errors=True) if os.path.isdir(path) and not os.path.islink(path) else os.remove(path)
    emptied[root] = {"gb_before": round(size / 1e9, 2), "left": sorted(os.listdir(root))}
print(json.dumps({"executors_ended": len(ended), "roots": emptied}))
