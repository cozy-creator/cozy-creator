# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.36", "msgspec>=0.19"]
# ///
"""A stand-in game for the vram-yield session: raw driver allocations from a process that is not cozy's, the way a
game takes the GPU (sessions/vram-yield.sh). One op per run (`op=<name> cfg=<json>`):
  mode    machine.mode in TensorD's config.toml (`shared`: a desktop's default), read at its next start
  start   a detached game: `gb` GiB in 256 MiB blocks, now, once the GPU is busy (`trigger: util`) or once a new
          executor opens its CUDA context (`trigger: executor`), each refused block asked again every 50 ms up to
          `give_up_s`, held `hold_s` (`shrink_to_gb` after `shrink_after_s`: it settles smaller and stays); waits
          `wait_s` for its allocation to end
  status  its record: `alloc_s` (trigger to all of it), `reached`, `fails`, NVML at trigger and at done
  stop    ends it
The game has a scrubbed environment (no executor scope token, so no executor end reaps it), a stop file and a hard
lifetime. Nothing here talks to the network."""
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import msgspec
from cozy_runtime.author import App

D = Path("/var/tmp/game")
KEEP = ("PATH", "HOME", "LANG", "LD_LIBRARY_PATH", "NVIDIA_DRIVER_CAPABILITIES", "NVIDIA_VISIBLE_DEVICES")

COMMON = r'''
import ctypes, glob, json, os, sys, time
GiB, MiB = 1 << 30, 1 << 20

class _Util(ctypes.Structure):
    _fields_ = [("gpu", ctypes.c_uint), ("memory", ctypes.c_uint)]

class _Mem(ctypes.Structure):
    _fields_ = [("total", ctypes.c_ulonglong), ("free", ctypes.c_ulonglong), ("used", ctypes.c_ulonglong)]

class Nvml:
    def __init__(self):
        self.lib = ctypes.CDLL("libnvidia-ml.so.1")
        self.lib.nvmlInit_v2()
        self.h = ctypes.c_void_p()
        self.lib.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(self.h))
    def util(self):
        u = _Util(); self.lib.nvmlDeviceGetUtilizationRates(self.h, ctypes.byref(u)); return u.gpu
    def mem(self):
        m = _Mem(); self.lib.nvmlDeviceGetMemoryInfo(self.h, ctypes.byref(m)); return m

def write(path, obj):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        f.write(json.dumps(obj))
    os.replace(tmp, path)

def cmdline(pid):
    try:
        return open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace").strip()
    except OSError:
        return ""

def ppid(pid):
    try:
        return int(open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[1])
    except (OSError, IndexError, ValueError):
        return 0

def cpu_ticks(pid):
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return int(f[11]) + int(f[12])
    except (OSError, IndexError, ValueError):
        return 0

def rss_mib(pid):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) >> 10
    except OSError:
        pass
    return 0

def exe(pid):
    try:
        return os.path.basename(os.readlink(f"/proc/{pid}/exe"))
    except OSError:
        return ""

def gpu_fds(pid):
    n = 0
    try:
        for fd in os.listdir(f"/proc/{pid}/fd"):
            try:
                if os.readlink(f"/proc/{pid}/fd/{fd}").startswith("/dev/nvidia"):
                    n += 1
            except OSError:
                pass
    except OSError:
        return -1
    return n

def pids():
    return [int(p) for p in os.listdir("/proc") if p.isdigit()]

def cozy_bin(pid):
    e = exe(pid)
    return e.startswith("cozy") or e.startswith("tensord")

def tensord():
    """The TensorD service: a cozy-binary process whose parent is the cozy-binary stable parent."""
    bins = {p for p in pids() if cozy_bin(p)}
    return sorted(p for p in bins if ppid(p) in bins)

def executors(gpu_only=True):
    out = []
    for p in pids():
        c = cmdline(p)
        if "cozy_runtime.internal.executor" in c and (not gpu_only or gpu_fds(p) > 0):
            out.append(p)
    return sorted(out)

def meminfo():
    m = {}
    for line in open("/proc/meminfo"):
        k, v = line.split(":", 1)
        m[k] = int(v.split()[0]) >> 10
    return m

def cg_current():
    for f in ("/sys/fs/cgroup/memory.current", "/sys/fs/cgroup/memory/memory.usage_in_bytes"):
        try:
            return int(open(f).read()) >> 20
        except (OSError, ValueError):
            pass
    return -1

def wait_util(nv, pct, secs, stop, deadline):
    run, last = 0.0, time.time()
    while True:
        if os.path.exists(stop):
            return "stopped"
        if time.time() > deadline:
            return "timeout"
        u, t = nv.util(), time.time()
        run = run + (t - last) if u >= pct else 0.0
        last = t
        if run >= secs:
            return "fired"
        time.sleep(0.1)
'''

HOLDER = COMMON + r'''
cfg = json.loads(sys.argv[1]); d = cfg["dir"]; name = cfg["name"]
st, stop = f"{d}/game-{name}.json", f"{d}/game-{name}.stop"
nv = Nvml()
S = {"pid": os.getpid(), "name": name, "cfg": cfg, "phase": "armed", "t_armed": time.time(), "marks": []}
write(st, S)
if cfg.get("trigger") == "util":
    r = wait_util(nv, cfg["util_pct"], cfg["util_s"], stop, time.time() + cfg.get("trigger_timeout_s", 1200))
    if r != "fired":
        S["phase"] = "trigger_" + r; write(st, S); sys.exit(0)
elif cfg.get("trigger") == "executor":  # a new GPU executor appears: its CUDA context is opening
    base, deadline = set(executors()), time.time() + cfg.get("trigger_timeout_s", 1200)
    while not [p for p in executors() if p not in base]:
        if os.path.exists(stop) or time.time() > deadline:
            S["phase"] = "trigger_timeout"; write(st, S); sys.exit(0)
        time.sleep(0.02)
time.sleep(cfg.get("delay_s", 0))
m = nv.mem()
t0 = time.time()
S.update(phase="allocating", t_trigger=t0, nvml_at_trigger={"used_mib": m.used >> 20, "free_mib": m.free >> 20, "total_mib": m.total >> 20})
write(st, S)
cu = ctypes.CDLL("libcuda.so.1")
cu.cuMemAlloc_v2.argtypes = [ctypes.POINTER(ctypes.c_ulonglong), ctypes.c_size_t]
cu.cuMemsetD8_v2.argtypes = [ctypes.c_ulonglong, ctypes.c_ubyte, ctypes.c_size_t]
cu.cuMemFree_v2.argtypes = [ctypes.c_ulonglong]
cu.cuMemGetInfo_v2.argtypes = [ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
give_up = t0 + cfg.get("give_up_s", 60)
fails, ctx_fails = 0, 0
rc = cu.cuInit(0)
dev = ctypes.c_int(); cu.cuDeviceGet(ctypes.byref(dev), 0)
ctx = ctypes.c_void_p()
while True:  # a full card can refuse the game its context too
    rc = cu.cuDevicePrimaryCtxRetain(ctypes.byref(ctx), dev)
    if rc == 0:
        break
    ctx_fails += 1
    if time.time() > give_up:
        S.update(phase=f"no_context rc={rc}", ctx_fails=ctx_fails, t_done=time.time()); write(st, S); sys.exit(0)
    time.sleep(cfg.get("retry_s", 0.05))
cu.cuCtxSetCurrent(ctx)
S["ctx_s"] = round(time.time() - t0, 3); S["ctx_fails"] = ctx_fails
block = cfg.get("block_mib", 256) * MiB
target = int(cfg["gb"] * GiB) if cfg.get("mode", "gb") == "gb" else None
leave = int(cfg.get("leave_gb", 0) * GiB)
held, total, mark = [], 0, GiB
while True:
    if os.path.exists(stop):
        S["phase"] = "stopped_while_allocating"; break
    if target is not None:
        if total >= target:
            S["phase"] = "reached"; S["reached"] = True; break
        size = min(block, target - total)
    else:
        f, t = ctypes.c_size_t(), ctypes.c_size_t()
        cu.cuMemGetInfo_v2(ctypes.byref(f), ctypes.byref(t))
        want = f.value - leave
        if want < 16 * MiB:
            S["phase"] = "reached"; S["reached"] = True; break
        size = min(block, want) // (2 * MiB) * (2 * MiB)
    p = ctypes.c_ulonglong()
    rc = cu.cuMemAlloc_v2(ctypes.byref(p), size)
    if rc == 0:
        cu.cuMemsetD8_v2(p.value, 0x5A, size)
        held.append((p.value, size)); total += size
        if total >= mark:
            S["marks"].append([round(time.time() - t0, 3), total >> 20, fails]); mark += GiB; write(st, S)
    elif rc == 2:
        fails += 1
        S.setdefault("first_fail_s", round(time.time() - t0, 3))
        if time.time() > give_up:
            S["phase"] = "gave_up"; break
        time.sleep(cfg.get("retry_s", 0.05))
    else:
        S["phase"] = f"error rc={rc}"; break
cu.cuCtxSynchronize()
m = nv.mem()
S.update(held_mib=total >> 20, fails=fails, t_done=time.time(), alloc_s=round(time.time() - t0, 3),
         nvml_at_done={"used_mib": m.used >> 20, "free_mib": m.free >> 20})
write(st, S)
end = time.time() + cfg.get("hold_s", 1800)
if cfg.get("shrink_to_gb") is not None:  # it settles smaller and stays
    shrink_at = time.time() + cfg.get("shrink_after_s", 10)
    while not os.path.exists(stop) and time.time() < shrink_at:
        time.sleep(0.2)
    keep = int(cfg["shrink_to_gb"] * GiB)
    while held and total > keep:
        p, size = held.pop(); cu.cuMemFree_v2(p); total -= size
    cu.cuCtxSynchronize()
    m = nv.mem()
    S.update(phase="shrunk", held_mib=total >> 20, t_shrunk=time.time(), nvml_at_shrunk={"used_mib": m.used >> 20, "free_mib": m.free >> 20})
    write(st, S)
while not os.path.exists(stop) and time.time() < end:
    time.sleep(0.2)
for p, _ in held:
    cu.cuMemFree_v2(p)
cu.cuDevicePrimaryCtxRelease(dev)
S.update(phase="released", t_released=time.time(), released_by="stop" if os.path.exists(stop) else "hold_s")
write(st, S)
'''

def env(gpu: bool) -> dict:
    e = {k: v for k, v in os.environ.items() if k in KEEP}
    if gpu:
        e["CUDA_VISIBLE_DEVICES"] = "0"
    return e


def daemon(code: str, cfg: dict, gpu: bool, log: str) -> int:
    path = D / f"{log}.py"
    path.write_text(code)
    with open(D / f"{log}.log", "ab") as f:
        p = subprocess.Popen([sys.executable, "-I", str(path), json.dumps(cfg | {"dir": str(D)})], env=env(gpu), stdout=f,
                             stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True)
    return p.pid


def alive(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except (OSError, IndexError):
        return False


def load(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def stop_and_wait(state: Path, stopfile: Path, secs: float = 30) -> dict:
    s = load(state)
    stopfile.touch()
    t = time.time()
    while s.get("pid") and alive(s["pid"]) and time.time() - t < secs:
        time.sleep(0.2)
    return load(state)


def mode(cfg: dict) -> dict:
    """`machine.mode` in TensorD's config.toml (its COZY_HOME, else HOME/.cozy)."""
    home = Path.home() / ".cozy"
    for proc in Path("/proc").iterdir():
        try:
            if not os.readlink(proc / "exe").rsplit("/", 1)[-1].startswith("cozy"):
                continue
            env = dict(kv.split("=", 1) for kv in (proc / "environ").read_text().split("\0") if "=" in kv)
        except OSError:
            continue
        home = Path(env.get("COZY_HOME") or Path(env.get("HOME", str(Path.home()))) / ".cozy")
        break
    path = home / "config.toml"
    text = path.read_text() if path.exists() else ""
    out, section, done = [], "", False
    for line in text.splitlines():
        if line.strip().startswith("["):
            if section == "[machine]" and not done:
                out.append(f'mode = "{cfg["mode"]}"')
                done = True
            section = line.strip()
        if section == "[machine]" and line.strip().startswith("mode") and "=" in line:
            if not done:
                out.append(f'mode = "{cfg["mode"]}"')
                done = True
            continue
        out.append(line)
    if not done:
        out += [f'mode = "{cfg["mode"]}"'] if section == "[machine]" else ["", "[machine]", f'mode = "{cfg["mode"]}"']
    path.write_text("\n".join(out) + "\n")
    return {"path": str(path), "after": path.read_text()[-2000:]}


def run(cfg: dict) -> dict:
    op, name = cfg["op"], cfg.get("name", "game")
    st, stop = D / f"game-{name}.json", D / f"game-{name}.stop"
    if op == "mode":
        return mode(cfg)
    if op == "start":
        if load(st).get("pid") and alive(load(st)["pid"]):
            stop_and_wait(st, stop)
        stop.unlink(missing_ok=True)
        st.unlink(missing_ok=True)
        pid = daemon(HOLDER, cfg, True, f"holder-{name}")
        t, s = time.time(), {}
        while time.time() - t < cfg.get("wait_s", 0):
            s = load(st)
            if s.get("t_done") or not alive(pid):
                break
            time.sleep(0.2)
        s = load(st) or {"pid": pid}
        if not alive(pid) and "t_done" not in s and not str(s.get("phase", "")).startswith("trigger"):
            s["log"] = (D / f"holder-{name}.log").read_text()[-1500:]
        return s
    if op == "status":
        return load(st)
    if op == "stop":
        return stop_and_wait(st, stop)
    raise ValueError(f"unknown op {op}")


app = App()


class Op(msgspec.Struct, forbid_unknown_fields=True):
    op: str
    cfg: str = "{}"


class Value(msgspec.Struct):
    value: str


@app.entrypoint(accelerator=False)
def main(payload: Op) -> Value:
    D.mkdir(parents=True, exist_ok=True)
    return Value(json.dumps(run(json.loads(payload.cfg) | {"op": payload.op})))
