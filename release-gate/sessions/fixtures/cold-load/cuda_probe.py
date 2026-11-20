"""cl-1327's CUDA-init probe (proof only, never merged): where an executor's fork and device start spend their time, beside a
plain torch process on the same pod. Run as root over `cozy machine ssh`, one JSON line per mode:
  patch    every environment's Runtime (`cozy_runtime/internal/{executor,accel,zygote,residency}.py`) gets timing lines around
           its device start's steps, printed to the executor's stderr as `[cl1327-probe] …`; the originals are kept beside them
  collect  every `[cl1327-probe]` line the executors printed, the import-only parents' mapped CUDA libraries and memory, the
           executors' CUDA JIT cache, and the memory log's host events
  restore  the originals back
  plain <bare|preload|nvml>   a fresh process of an environment's Python: torch's import, CUDA init and the executor's first
           launches (mm, conv2d, conv3d per dtype), each synchronized; preload dlopens cuDNN and cuBLAS before CUDA init, as an
           import-only parent could; nvml times NVML's init and queries first
"""

import glob
import json
import os
import re
import subprocess
import sys

COZY = "/var/lib/cozy"
GPU = f"{COZY}/tensord/gpu"
SAVED = ".cl1327-orig"

# (file, function start, function end, [(anchor line, "before" | "after", inserted lines)])
MARK = "_cl1327_mark"
INIT = [
    ("        owner = self.host\n", "after",
     "        import time as _cl1327_t\n"
     "        _cl1327_c, _cl1327_legs = [_cl1327_t.perf_counter()], []\n"
     "        def _cl1327_mark(name):\n"
     "            now = _cl1327_t.perf_counter()\n"
     "            _cl1327_legs.append((name, round((now - _cl1327_c[0]) * 1000, 1)))\n"
     "            _cl1327_c[0] = now\n"),
    ("        if self._device_ready:\n", "before", f"        {MARK}('startup_facts')\n"),
    ("        contract.pin(torch)\n", "after", f"        {MARK}('numerics')\n"),
    ("            accel.bind_allocator(torch, self.device_kind)\n", "after", f"            {MARK}('bind_allocator')\n"),
    ("            accel.initialize(torch, self.device_kind)\n", "before", f"            {MARK}('varena_activate')\n"),
    ("            accel.initialize(torch, self.device_kind)\n", "after", f"            {MARK}('torch_cuda_init')\n"),
    ("            self._open_memory(torch)\n", "before", f"            {MARK}('set_device_stream')\n"),
    ("            self._open_memory(torch)\n", "after", f"            {MARK}('open_memory')\n"),
    ("        accel.first_launches(torch, self.device_kind)\n", "before", f"        {MARK}('boundary')\n"),
    ("        accel.first_launches(torch, self.device_kind)\n", "after", f"        {MARK}('first_launches')\n"),
    ("        place(self._device())\n", "after",
     f"        {MARK}('place')\n"
     "        print('[cl1327-probe] init ' + json.dumps(_cl1327_legs), file=sys.stderr, flush=True)\n"),
]
LAUNCHES_OLD = """        for dtype in (torch.bfloat16, torch.float16, torch.float32):
            a = torch.ones(8, 8, dtype=dtype, device="cuda")
            kernel = a[:3, :3]
            with suppress(RuntimeError):  # a card without this dtype
                torch.mm(a, a)
                conv.conv2d(a.view(1, 1, 8, 8), kernel.reshape(1, 1, 3, 3))
                conv.conv3d(a.view(1, 1, 1, 8, 8), kernel.reshape(1, 1, 1, 3, 3))
        completion_event(torch, kind).synchronize()
"""
LAUNCHES_NEW = """        import json as _j, sys as _s, time as _t
        _legs, _c = [], [_t.perf_counter()]
        def _mark(name):
            torch.cuda.synchronize()
            now = _t.perf_counter()
            _legs.append((name, round((now - _c[0]) * 1000, 1)))
            _c[0] = now
        for dtype in (torch.bfloat16, torch.float16, torch.float32):
            d = str(dtype).rsplit(".", 1)[-1]
            a = torch.ones(8, 8, dtype=dtype, device="cuda")
            kernel = a[:3, :3]
            _mark("ones_" + d)
            with suppress(RuntimeError):  # a card without this dtype
                torch.mm(a, a)
                _mark("mm_" + d)
                conv.conv2d(a.view(1, 1, 8, 8), kernel.reshape(1, 1, 3, 3))
                _mark("conv2d_" + d)
                conv.conv3d(a.view(1, 1, 1, 8, 8), kernel.reshape(1, 1, 1, 3, 3))
                _mark("conv3d_" + d)
        completion_event(torch, kind).synchronize()
        print("[cl1327-probe] first_launches " + _j.dumps(_legs), file=_s.stderr, flush=True)
"""
FORK_OLD = """    gc.collect()
    gc.freeze()  # imported objects stay shared: the collector never writes their pages
    parent = os.getpid()
    pid = os.fork()
    if pid:
        return pid
"""
FORK_NEW = """    import time as _t
    _a = _t.perf_counter()
    gc.collect()
    _b = _t.perf_counter()
    gc.freeze()  # imported objects stay shared: the collector never writes their pages
    _c = _t.perf_counter()
    parent = os.getpid()
    pid = os.fork()
    if pid:
        _d = _t.perf_counter()
        print(f"[cl1327-probe] fork gc_collect={(_b - _a) * 1000:.1f} gc_freeze={(_c - _b) * 1000:.1f} os_fork={(_d - _c) * 1000:.1f} ms",
              file=sys.stderr, flush=True)
        return pid
"""
MEMORY_OLD = """                self.native = varena.Device(self.device.index or 0)
                self.granularity = int(self.native.granularity)
                self.pages = _pages(self.device.index or 0, shared)
"""
MEMORY_NEW = """                import sys as _s, time as _t
                _a = _t.perf_counter()
                self.native = varena.Device(self.device.index or 0)
                _b = _t.perf_counter()
                self.granularity = int(self.native.granularity)
                self.pages = _pages(self.device.index or 0, shared)
                _c = _t.perf_counter()
                print(f"[cl1327-probe] memory varena_device={(_b - _a) * 1000:.1f} host_pages={(_c - _b) * 1000:.1f} ms",
                      file=_s.stderr, flush=True)
"""


def runtimes() -> list[str]:
    """Each environment's `cozy_runtime/internal` directory."""
    found = subprocess.run(["find", COZY, "-xdev", "-path", "*/site-packages/cozy_runtime/internal/executor.py"],
                           capture_output=True, text=True).stdout.split()
    return sorted({os.path.dirname(p) for p in found})


def patched_init(text: str) -> str:
    start = text.index("    def _initialize_device(self, torch: ModuleType) -> Reply | None:\n")
    end = text.index("\n    def ", start + 10)
    body = text[start:end]
    for anchor, where, lines in INIT:
        if body.count(anchor) != 1:
            raise ValueError(f"executor.py: {anchor.strip()!r} found {body.count(anchor)} times")
        body = body.replace(anchor, lines + anchor if where == "before" else anchor + lines)
    return text[:start] + body + text[end:]


def swap(path: str, old: str, new: str) -> str:
    text = open(path).read()
    if text.count(old) != 1:
        raise ValueError(f"{os.path.basename(path)}: its block found {text.count(old)} times")
    return text.replace(old, new)


def patch() -> dict[str, object]:
    done, failed = [], []
    for d in runtimes():
        try:
            texts = {
                "executor.py": patched_init(open(f"{d}/executor.py").read()),
                "accel.py": swap(f"{d}/accel.py", LAUNCHES_OLD, LAUNCHES_NEW),
                "zygote.py": swap(f"{d}/zygote.py", FORK_OLD, FORK_NEW),
                "residency.py": swap(f"{d}/residency.py", MEMORY_OLD, MEMORY_NEW),
            }
            for name, text in texts.items():
                compile(text, name, "exec")
        except (OSError, ValueError, SyntaxError) as err:
            failed.append(f"{d}: {err}")
            continue
        for name, text in texts.items():
            path = f"{d}/{name}"
            if not os.path.exists(path + SAVED):
                os.replace(path, path + SAVED)
            tmp = path + ".cl1327-tmp"
            with open(tmp, "w") as out:
                out.write(text)
            os.chmod(tmp, os.stat(path + SAVED).st_mode)
            os.replace(tmp, path)  # a new inode: a hard link to uv's cache keeps the original
            for pyc in glob.glob(f"{d}/__pycache__/{name[:-3]}.*.pyc"):
                os.replace(pyc, pyc + SAVED)
        done.append(d)
    return {"patched": done, "failed": failed}


def restore() -> dict[str, object]:
    back = []
    for d in runtimes():
        for saved in glob.glob(f"{d}/*{SAVED}") + glob.glob(f"{d}/__pycache__/*{SAVED}"):
            os.replace(saved, saved[: -len(SAVED)])
            back.append(saved[: -len(SAVED)])
    return {"restored": back}


def executors() -> list[int]:
    found = []
    for pid in os.listdir("/proc"):
        try:
            if pid.isdigit() and b"cozy_runtime.internal.executor" in open(f"/proc/{pid}/cmdline", "rb").read():
                found.append(int(pid))
        except OSError:
            continue
    return sorted(found)


def nvidia_fds(pid: int) -> int:
    n = 0
    for fd in os.listdir(f"/proc/{pid}/fd"):
        try:
            n += os.readlink(f"/proc/{pid}/fd/{fd}").startswith("/dev/nvidia")
        except OSError:
            continue
    return n


def rss_gb(pid: int) -> float:
    for line in open(f"/proc/{pid}/status"):
        if line.startswith("VmRSS:"):
            return round(int(line.split()[1]) * 1024 / 1e9, 3)
    return 0.0


def collect() -> dict[str, object]:
    lines = []
    for log in glob.glob(f"{GPU}/*/*.log") + glob.glob(f"{GPU}/*/*/*.log"):
        try:
            for line in open(log, errors="replace"):
                if "[cl1327-probe]" in line:
                    lines.append({"log": log, "mtime": int(os.path.getmtime(log)), "line": line.strip()})
        except OSError:
            continue
    procs = []
    for pid in executors():
        try:
            libs = sorted({os.path.basename(m.split()[-1]) for m in open(f"/proc/{pid}/maps")
                           if re.search(r"lib(cudnn|cublas|cudart|nvrtc|nvJitLink|cusparse|cufft|curand|nccl|cuda)[^/]*\.so", m)})
            procs.append({"pid": pid, "python": open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")[0].decode(),
                          "nvidia_fds": nvidia_fds(pid), "rss_gb": rss_gb(pid), "threads": len(os.listdir(f"/proc/{pid}/task")),
                          "cuda_libs": libs})
        except OSError:
            continue
    jit = subprocess.run(["sh", "-c", f"du -sb {GPU}/jit/*/*/cuda 2>/dev/null; find {GPU}/jit -path '*/cuda/*' -type f 2>/dev/null | wc -l"],
                         capture_output=True, text=True).stdout.split("\n")
    events = []
    for log in subprocess.run(["find", "/var/lib", "-xdev", "-name", "memory.log", "-path", "*logs*"], capture_output=True,
                              text=True).stdout.split():
        for line in open(log, errors="replace"):
            if re.search(r'"event": ?"(host_short|host_asked|import_installed)"', line):
                events.append(line.strip()[:400])
    return {"probe": lines, "executors": procs, "cuda_jit_cache": jit, "memory_events": events[-40:]}


PLAIN = r"""
import ctypes, glob, json, os, sys, time
mode = sys.argv[1]
out = {"mode": mode}
t = time.perf_counter()
import torch
out["import_torch_s"] = round(time.perf_counter() - t, 3)
out["torch"] = torch.__version__
site = os.path.dirname(os.path.dirname(torch.__file__))
def fds():
    n = []
    for fd in os.listdir("/proc/self/fd"):
        try:
            p = os.readlink(f"/proc/self/fd/{fd}")
        except OSError:
            continue
        if p.startswith("/dev/nvidia"):
            n.append(p)
    return n
out["mapped_at_import"] = sorted({os.path.basename(m.split()[-1]) for m in open("/proc/self/maps") if ".so" in m and ("cudnn" in m or "cublas" in m)})
if mode == "preload":
    libs = []
    for pat in ("nvidia/cudnn/lib/libcudnn*.so*", "nvidia/cublas/lib/libcublas*.so*"):
        libs += sorted(glob.glob(os.path.join(site, pat)))
    t = time.perf_counter()
    for p in libs:
        try:
            ctypes.CDLL(p, mode=ctypes.RTLD_GLOBAL)
        except OSError as e:
            out.setdefault("preload_failed", []).append(f"{os.path.basename(p)}: {e}")
    out["preload_s"] = round(time.perf_counter() - t, 3)
    out["preloaded"] = [os.path.basename(p) for p in libs]
    out["after_preload"] = {"cuda_initialized": torch.cuda.is_initialized(), "nvidia_fds": fds(), "threads": len(os.listdir("/proc/self/task"))}
if mode == "nvml":
    t = time.perf_counter()
    nvml = ctypes.CDLL("libnvidia-ml.so.1")
    out["nvml_load_s"] = round(time.perf_counter() - t, 3)
    t = time.perf_counter()
    out["nvml_rc"] = nvml.nvmlInit_v2()
    out["nvml_init_s"] = round(time.perf_counter() - t, 3)
    h = ctypes.c_void_p()
    class M(ctypes.Structure):
        _fields_ = [("total", ctypes.c_ulonglong), ("free", ctypes.c_ulonglong), ("used", ctypes.c_ulonglong)]
    m = M()
    t = time.perf_counter()
    nvml.nvmlDeviceGetHandleByIndex_v2(0, ctypes.byref(h))
    nvml.nvmlDeviceGetMemoryInfo(h, ctypes.byref(m))
    out["nvml_memory_s"] = round(time.perf_counter() - t, 4)
    n = ctypes.c_uint(0)
    t = time.perf_counter()
    nvml.nvmlDeviceGetComputeRunningProcesses_v3(h, ctypes.byref(n), None)
    out["nvml_processes_s"] = round(time.perf_counter() - t, 4)
t = time.perf_counter()
torch.cuda.init()
out["torch_cuda_init_s"] = round(time.perf_counter() - t, 3)
t = time.perf_counter()
torch.cuda.set_device(0)
x = torch.zeros(1, device="cuda")
torch.cuda.synchronize()
out["first_alloc_s"] = round(time.perf_counter() - t, 3)
legs = []
conv = torch.nn.functional
with torch.inference_mode():
    for dtype in (torch.bfloat16, torch.float16, torch.float32):
        a = torch.ones(8, 8, dtype=dtype, device="cuda")
        k = a[:3, :3]
        for name, fn in (("mm", lambda: torch.mm(a, a)), ("conv2d", lambda: conv.conv2d(a.view(1, 1, 8, 8), k.reshape(1, 1, 3, 3))),
                         ("conv3d", lambda: conv.conv3d(a.view(1, 1, 1, 8, 8), k.reshape(1, 1, 1, 3, 3)))):
            t = time.perf_counter()
            fn()
            torch.cuda.synchronize()
            legs.append((name + "_" + str(dtype).rsplit(".", 1)[-1], round((time.perf_counter() - t) * 1000, 1)))
out["first_launches_ms"] = legs
out["first_launches_s"] = round(sum(ms for _, ms in legs) / 1000, 3)
print(json.dumps(out))
"""


def python() -> str:
    """An environment's Python, SDXL's where it can tell: a live executor's, else one beside a Runtime."""
    found = []
    for pid in executors():
        try:
            found.append(open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0")[0].decode())
        except OSError:
            continue
    for d in runtimes():
        env = d
        for _ in range(5):  # internal, cozy_runtime, site-packages, python3.x, lib
            env = os.path.dirname(env)
        found.append(f"{env}/bin/python")
    found = [p for p in found if os.path.exists(p)]
    return next((p for p in found if "sdxl" in p), found[0] if found else sys.executable)


def plain(mode: str) -> dict[str, object]:
    env = {"PATH": "/usr/bin:/bin", "HOME": "/tmp", "CUDA_VISIBLE_DEVICES": "0", "CUDA_DEVICE_ORDER": "PCI_BUS_ID",
           "OMP_NUM_THREADS": "4"}
    py = python()
    done = subprocess.run([py, "-I", "-c", PLAIN, mode], capture_output=True, text=True, env=env, timeout=600)
    try:
        out = json.loads(done.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError):
        out = {"mode": mode, "failed": done.stderr[-800:]}
    out["python"] = py
    return out


mode = sys.argv[1]
if mode == "patch":
    said = patch()
elif mode == "restore":
    said = restore()
elif mode == "collect":
    said = collect()
elif mode == "plain":
    said = plain(sys.argv[2])
else:
    said = {"error": f"no mode {mode}"}
print(json.dumps({"mode": mode, **said}))
