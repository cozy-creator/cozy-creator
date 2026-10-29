# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.34", "msgspec>=0.19"]
# ///
"""On the rental, holding no GPU itself (lowvram.sh). One op per call:

- sampler: start the detached NVML sampler (idempotent): every 0.1 s, GPU 0's memory used and the H2D bytes
  integrated from nvmlDeviceGetPcieThroughput(RX). The same sampler measures both engines.
- hold: a separate non-Cozy process holds VRAM so that `free_mib` stays free, as a user's game would (0 releases).
- evict: drop the page cache over ComfyUI's models and the TensorFS store, so the next load reads the disk.
- mark: a labelled point on the sampler's clock: H2D bytes so far, peak memory used since the last mark.
- collect: the sampler's CSV and the marks.
"""
import ctypes
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import msgspec

from cozy_runtime.author import App, Context, FileAsset, Outputs, Telemetry
from cozy_runtime.internal.proctree import available_host_bytes

app = App()
D = Path("/var/tmp/lowvram")
COMFY_PY = "/var/tmp/lowvram/comfy-env/bin/python"
MAX_S = 4 * 3600

SAMPLER = r'''
import ctypes, sys, time
from pathlib import Path
d, max_s = Path(sys.argv[1]), float(sys.argv[2])
nvml = ctypes.CDLL("libnvidia-ml.so.1")
class Mem(ctypes.Structure):
    _fields_ = [("total", ctypes.c_ulonglong), ("free", ctypes.c_ulonglong), ("used", ctypes.c_ulonglong)]
nvml.nvmlInit_v2()
h = ctypes.c_void_p()
nvml.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(h))
kbps, mem, rx, last, t0 = ctypes.c_uint(), Mem(), 0.0, time.time(), time.time()
with open(d / "nvml.csv", "a", buffering=1) as out:
    if out.tell() == 0:
        out.write("unix_s,h2d_bytes,used_mib\n")
    while not (d / "stop").exists() and time.time() - t0 < max_s:
        ok = nvml.nvmlDeviceGetPcieThroughput(h, 1, ctypes.byref(kbps)) == 0  # RX KB/s over its 20 ms window
        now = time.time()
        if ok:
            rx += kbps.value * 1024 * (now - last)
        last = now
        nvml.nvmlDeviceGetMemoryInfo(h, ctypes.byref(mem))
        out.write(f"{now:.3f},{int(rx)},{mem.used >> 20}\n")
        time.sleep(0.08)
'''

HOLDER = r'''
import json, os, sys, time, torch
from pathlib import Path
d, target = Path(sys.argv[1]), int(sys.argv[2]) << 20
torch.cuda.init()
held, chunk = [], 256 << 20
while True:
    free, total = torch.cuda.mem_get_info()
    want = free - target
    if want < (8 << 20):
        break
    try:
        t = torch.empty(min(chunk, want), dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        chunk //= 2
        if chunk < (8 << 20):
            break
        continue
    t.fill_(1)
    held.append(t)
torch.cuda.synchronize()
free, total = torch.cuda.mem_get_info()
(d / "hold.json").write_text(json.dumps({"pid": os.getpid(), "held_mib": sum(t.numel() for t in held) >> 20,
                                         "free_mib": free >> 20, "total_mib": total >> 20, "target_mib": target >> 20}))
while not (d / "hold.stop").exists():
    time.sleep(1)
'''


class ToolRequest(msgspec.Struct):
    op: str
    label: str = ""
    free_mib: int = 0
    ram_free_gib: int = 0


class ToolResult(msgspec.Struct):
    value: str
    files: list[FileAsset] = msgspec.field(default_factory=list)


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return Path(f"/proc/{pid}/stat").read_text().split()[2] != "Z"
    except (OSError, IndexError, ValueError):
        return False


def smi() -> str:
    return subprocess.run(["nvidia-smi", "--query-gpu=memory.used,memory.free,memory.total", "--format=csv,noheader"],
                          capture_output=True, text=True).stdout.strip()


def gpu_env() -> dict:
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LD_LIBRARY_PATH", "NVIDIA_DRIVER_CAPABILITIES")}
    env["CUDA_VISIBLE_DEVICES"] = "0"
    return env


def rows() -> list[list[float]]:
    try:
        return [[float(x) for x in line.split(",")] for line in (D / "nvml.csv").read_text().splitlines()[1:] if line.count(",") == 2]
    except (OSError, ValueError):
        return []


def sampler() -> str:
    pid = int((D / "sampler.pid").read_text()) if (D / "sampler.pid").exists() else 0
    if pid and alive(pid):
        return f"sampler already running ({pid})"
    (D / "stop").unlink(missing_ok=True)
    (D / "sampler.py").write_text(SAMPLER)
    p = subprocess.Popen([sys.executable, str(D / "sampler.py"), str(D), str(MAX_S)], stdin=subprocess.DEVNULL,
                         stdout=open(D / "sampler.log", "ab"), stderr=subprocess.STDOUT, start_new_session=True)
    (D / "sampler.pid").write_text(str(p.pid))
    time.sleep(2)
    if len(rows()) < 2:
        raise RuntimeError(f"the sampler wrote nothing: {(D / 'sampler.log').read_text()[-600:]}")
    return f"sampler {p.pid}: {rows()[-1]}"


def hold(free_mib: int, tel: Telemetry) -> str:
    old = json.loads((D / "hold.json").read_text()) if (D / "hold.json").exists() else None
    if old and alive(old["pid"]):
        (D / "hold.stop").touch()
        for _ in range(120):
            if not alive(old["pid"]):
                break
            time.sleep(0.5)
    (D / "hold.stop").unlink(missing_ok=True)
    (D / "hold.json").unlink(missing_ok=True)
    if free_mib <= 0:
        time.sleep(2)
        return json.dumps({"released": True, "smi": smi()})
    (D / "holder.py").write_text(HOLDER)
    subprocess.Popen([COMFY_PY, str(D / "holder.py"), str(D), str(free_mib)], env=gpu_env(), stdin=subprocess.DEVNULL,
                     stdout=open(D / "holder.log", "ab"), stderr=subprocess.STDOUT, start_new_session=True)
    for _ in range(240):
        if (D / "hold.json").exists():
            break
        time.sleep(0.5)
    if not (D / "hold.json").exists():
        raise RuntimeError(f"the VRAM holder did not start: {(D / 'holder.log').read_text()[-800:]}")
    time.sleep(1)
    state = json.loads((D / "hold.json").read_text()) | {"smi": smi()}
    tel.log(json.dumps(state))
    return json.dumps(state)


PIN_PROBE = r'''
import json, os, sys, threading, time, torch
torch.cuda.init()
cudart = torch.cuda.cudart()
out = {"cpus": os.cpu_count(), "torch": torch.__version__}
def t(f):
    t0 = time.perf_counter(); r = f(); return time.perf_counter() - t0, r
for mib in (64, 256, 1024):
    n, rows = mib << 20, {}
    for _ in range(2):
        z, a = t(lambda: torch.zeros(n, dtype=torch.uint8)); rows.setdefault("zeros_s", []).append(round(z, 4))
        g, _c = t(lambda: cudart.cudaHostRegister(a.data_ptr(), n, 0)); rows.setdefault("register_touched_s", []).append(round(g, 4))
        d = torch.empty(n, dtype=torch.uint8, device="cuda")
        c, _ = t(lambda: (d.copy_(a, non_blocking=True), torch.cuda.synchronize())); rows.setdefault("h2d_s", []).append(round(c, 4))
        u, _ = t(lambda: cudart.cudaHostUnregister(a.data_ptr())); rows.setdefault("unregister_s", []).append(round(u, 4))
        del a, d
        e, b = t(lambda: torch.empty(n, dtype=torch.uint8)); rows.setdefault("empty_s", []).append(round(e, 4))
        g2, _ = t(lambda: cudart.cudaHostRegister(b.data_ptr(), n, 0)); rows.setdefault("register_untouched_s", []).append(round(g2, 4))
        cudart.cudaHostUnregister(b.data_ptr()); del b
        p, c2 = t(lambda: torch.empty(n, dtype=torch.uint8, pin_memory=True)); rows.setdefault("torch_pinned_s", []).append(round(p, 4))
        del c2
        # one register from a background thread while the main thread copies
        f = torch.empty(n, dtype=torch.uint8); f.fill_(1)
        done = {}
        th = threading.Thread(target=lambda: done.setdefault("rc", int(cudart.cudaHostRegister(f.data_ptr(), n, 0))))
        bg, _ = t(lambda: (th.start(), th.join())); rows.setdefault("register_thread_s", []).append(round(bg, 4))
        cudart.cudaHostUnregister(f.data_ptr()); del f
    out[f"{mib}MiB"] = {k: [round(n / 2**30 / max(x, 1e-9), 2) for x in v] for k, v in rows.items()}
    out[f"{mib}MiB_s"] = rows
print(json.dumps(out))
'''


RAM_HOLDER = r'''
import json, mmap, sys, time
from pathlib import Path
from cozy_runtime.internal.proctree import available_host_bytes as available  # the Runtime's cgroup-aware room
d, target = Path(sys.argv[1]), int(sys.argv[2]) << 30
print(f"available {available() / 2**30:.1f} GiB, target {target >> 30} GiB", flush=True)
held = []
while available() - target > (256 << 20):
    block = mmap.mmap(-1, min(available() - target, 1 << 30))
    for at in range(0, len(block), 4096):  # touch every page: anonymous memory, really taken
        block[at] = 1
    held.append(block)
(d / "ramhold.json").write_text(json.dumps({"held_gib": round(sum(len(b) for b in held) / 2**30, 2),
                                            "available_gib": round(available() / 2**30, 2), "target_gib": target >> 30}))
while not (d / "ramhold.stop").exists():
    time.sleep(0.5)
'''


def ramhold(free_gib: int, tel: Telemetry) -> str:
    """Hold host RAM so only `free_gib` stays available (a smaller machine), or release it (0)."""
    (D / "ramhold.stop").touch()
    time.sleep(1.5)
    (D / "ramhold.stop").unlink(missing_ok=True)
    (D / "ramhold.json").unlink(missing_ok=True)
    if free_gib <= 0:
        return json.dumps({"released": True})
    (D / "ramholder.py").write_text(RAM_HOLDER)
    subprocess.Popen([sys.executable, str(D / "ramholder.py"), str(D), str(free_gib)], stdin=subprocess.DEVNULL,
                     stdout=open(D / "ramholder.log", "ab"), stderr=subprocess.STDOUT, start_new_session=True)
    for _ in range(600):
        if (D / "ramhold.json").exists():
            break
        time.sleep(0.5)
    if not (D / "ramhold.json").exists():
        raise RuntimeError(f"the RAM holder did not start: {(D / 'ramholder.log').read_text()[-800:]}")
    state = (D / "ramhold.json").read_text()
    tel.log(state)
    return state


def stacks() -> str:
    """Every Cozy executor's thread stacks (py-spy; else faulthandler through SIGABRT, read back from
    its stderr file), with each thread's kernel wait channel: the stall's evidence."""
    out = []
    pids = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        try:
            cmd = (proc / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            continue
        if "cozy_runtime" in cmd and "python" in cmd:
            pids.append((int(proc.name), cmd[:200]))
    spy = subprocess.run([sys.executable, "-m", "pip", "install", "-q", "py-spy"], capture_output=True, text=True)
    for pid, cmd in pids:
        out.append(f"=== pid {pid}: {cmd}")
        for task in sorted(Path(f"/proc/{pid}/task").glob("*")):
            try:
                out.append(f"  tid {task.name} {(task / 'comm').read_text().strip()} wchan={(task / 'wchan').read_text().strip()}")
            except OSError:
                pass
        if spy.returncode == 0:
            dump = subprocess.run([str(Path(sys.executable).parent / "py-spy"), "dump", "--pid", str(pid), "--native"],
                                  capture_output=True, text=True, timeout=120)
            out.append(dump.stdout[-20000:] or dump.stderr[-2000:])
    return "\n".join(out)


LIBC = ctypes.CDLL("libc.so.6", use_errno=True)
LIBC.mmap.restype = ctypes.c_void_p
LIBC.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
LIBC.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]
LIBC.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]


def resident(path: Path) -> int:
    size = path.stat().st_size
    fd = os.open(path, os.O_RDONLY)
    try:
        addr = LIBC.mmap(None, size, 1, 1, fd, 0)
        if addr in (None, ctypes.c_void_p(-1).value):
            return 0
        vec = ctypes.create_string_buffer((size + 4095) // 4096)
        rc = LIBC.mincore(addr, size, vec)
        LIBC.munmap(addr, size)
        return 0 if rc else sum(b & 1 for b in vec.raw) * 4096
    finally:
        os.close(fd)


def weight_files() -> list[Path]:
    home = Path(os.environ.get("HOME", "/root"))
    roots = {D / "ComfyUI/models", Path(os.environ.get("TENSORFS_HOME") or home / ".cache/tensorfs"), home / ".cache/tensorfs"}
    return [p for r in roots if r.exists() for p in r.rglob("*")
            if p.is_file() and not p.is_symlink() and p.stat().st_size > (1 << 20)]


def evict() -> str:
    files = weight_files()
    before = sum(resident(p) for p in files)
    os.sync()
    try:
        Path("/proc/sys/vm/drop_caches").write_text("3\n")
        dropped = True
    except OSError:
        dropped = False
    for path in files:
        try:
            fd = os.open(path, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
    return json.dumps({"files": len(files), "gb": round(sum(p.stat().st_size for p in files) / 1e9, 2),
                       "resident_gb_before": round(before / 1e9, 2),
                       "resident_gb_after": round(sum(resident(p) for p in files) / 1e9, 2), "drop_caches": dropped,
                       "meminfo": {k: v for k, v in (line.split(":") for line in open("/proc/meminfo"))
                                   if k in ("MemTotal", "MemAvailable", "Cached")}})


def mark(label: str) -> str:
    """Marks are cut on the sampler's clock: a mark's window runs from the previous mark."""
    r = rows()
    marks = [json.loads(x) for x in (D / "marks.jsonl").read_text().splitlines()] if (D / "marks.jsonl").exists() else []
    since = marks[-1]["unix_s"] if marks else 0.0
    win = [x for x in r if x[0] > since]
    rec = {"label": label, "unix_s": time.time(), "h2d_bytes": int(r[-1][1]) if r else -1,
           "peak_used_mib": int(max((x[2] for x in win), default=-1)), "used_mib": int(r[-1][2]) if r else -1,
           "holder": json.loads((D / "hold.json").read_text()) if (D / "hold.json").exists() else None,
           "ram_available_gib": round(available_host_bytes() / 2**30, 2),
           "ramholder": json.loads((D / "ramhold.json").read_text()) if (D / "ramhold.json").exists() else None}
    with open(D / "marks.jsonl", "a") as f:
        f.write(json.dumps(rec) + "\n")
    return json.dumps(rec)


@app.entrypoint(accelerator=False)
def main(payload: ToolRequest, ctx: Context, tel: Telemetry, out: Outputs) -> ToolResult:
    D.mkdir(parents=True, exist_ok=True)
    if payload.op == "sampler":
        return ToolResult(sampler())
    if payload.op == "hold":
        return ToolResult(hold(payload.free_mib, tel))
    if payload.op == "ramhold":
        return ToolResult(ramhold(payload.ram_free_gib, tel))
    if payload.op == "pinprobe":  # the first Python on the pod with torch: the image's, else a TensorD env's
        pys = [*map(str, sorted(Path("/var/lib/cozy/tensord/generations").glob("*/env/bin/python"))), "/usr/local/bin/python3",
               "/usr/bin/python3", "/opt/conda/bin/python", COMFY_PY]
        py = next((p for p in pys if Path(p).exists() and subprocess.run([p, "-c", "import torch"], capture_output=True).returncode == 0), None)
        if py is None:
            return ToolResult("no python with torch: " + ", ".join(pys))
        res = subprocess.run([py, "-c", PIN_PROBE], capture_output=True, text=True, env=gpu_env())
        tel.log(res.stderr[-2000:])
        return ToolResult(res.stdout.strip() or res.stderr[-2000:])
    if payload.op == "stacks":
        (D / "stacks.txt").write_text(stacks())
        return ToolResult("stacks", [out.save_file(D / "stacks.txt", media_type="text/plain")])
    if payload.op == "evict":
        return ToolResult(evict())
    if payload.op == "mark":
        return ToolResult(mark(payload.label))
    if payload.op == "collect":
        (D / "stop").touch()
        time.sleep(1)
        files = [out.save_file(D / n, media_type="text/plain") for n in ("nvml.csv", "marks.jsonl") if (D / n).exists()]
        return ToolResult("collected", files)
    raise ValueError(f"no op {payload.op}")
