"""Suite bench's reference workload, run before and after a group's cells (cells.sh): fixed bf16 kernels (a GEMM, attention,
a convolution), the pinned host-to-device copy rate, a cold disk read, and the host CPU's own speed (single-thread Python,
10k tiny kernel launches, a CLIP-style tokenize). Within the package contract: no subprocess, fork, CUDA events or driver
reads. The GPU's clock and temperature come from TensorD's reads of this call (its record), its executor's fork and CUDA
init from the record's legs, and the GPU's name, driver, PCIe link and power limit from pod.sh `facts` (bench.py).
bench.py admit holds it against the GPU type's stored one: a pod off by more than its range is another class of pod, and is
replaced. Eager steps are host-bound: a 2-4x slower host CPU (euclase, 11-10) stepped SDXL at 193 ms against 120."""

import os
import re
import statistics
import sys
import tempfile
import time
from collections.abc import Callable

import msgspec
import torch
import torch.nn.functional as F

from cozy_runtime import rng
from cozy_runtime.author import App, device

app = App()
GIB = 1 << 30


class Calibrate(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 0


class Measured(msgspec.Struct):
    cpu: str
    cores: int  # the CPUs this process may run on
    cpu_limit: float | None  # the container's CPU quota in cores, when it has one
    host_gib: float  # MemTotal
    limit_gib: float | None  # the container's memory limit, cgroup v2 or v1, when under MemTotal
    gemm_ms: float
    attention_ms: float
    conv_ms: float
    h2d_gbps: float
    disk_gbps: float
    python_ms: float  # a fixed single-thread pure-Python workload
    launch_us: float  # one tiny CUDA kernel launch, over 10k: torch's dispatch and the driver's launch, host-bound
    tokenize_ms: float  # a fixed CLIP-style regex tokenize, 200 times


def timed(fn: Callable[[], object], dev: torch.device, runs: int = 20, batch: int = 10) -> float:
    """The median of `runs` samples in ms per call, each `batch` calls between two device waits, after three warm-ups."""
    for _ in range(3):
        fn()
    torch.cuda.synchronize(dev)
    times: list[float] = []
    for _ in range(runs):
        start = time.perf_counter()
        for _ in range(batch):
            fn()
        torch.cuda.synchronize(dev)
        times.append((time.perf_counter() - start) * 1000 / batch)
    return round(statistics.median(times), 3)


def wall(fn: Callable[[], object], runs: int = 5) -> float:
    """The median wall time of `runs` calls in ms, after one warm-up."""
    fn()
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        fn()
        times.append((time.perf_counter() - start) * 1000)
    return round(statistics.median(times), 3)


def python_work() -> int:
    d: dict[str, int] = {}
    for i in range(200_000):
        k = f'k{i % 5000}'
        d[k] = d.get(k, 0) + (i * 7919) % 101
    return sum(sorted(d.values()))


# CLIP's tokenizer pattern, ASCII form, and a fixed prompt.
WORDS = re.compile(r"""<\|startoftext\|>|<\|endoftext\|>|'s|'t|'re|'ve|'m|'ll|'d|[a-zA-Z]+|[0-9]|[^\sa-zA-Z0-9]+""", re.IGNORECASE)
PROMPT = ('An elderly astronomer in an indigo robe beside a wooden telescope on a desert dune at night, sand dunes under a '
          'starry sky with a lone tent, film still, 35 mm, soft rim light, fine grain, shallow depth of field, 1970s.')


def tokenize() -> int:
    n = 0
    for _ in range(200):
        n += sum(len(w.encode('utf-8')) for w in WORDS.findall(' '.join(PROMPT.lower().split())))
    return n


def cpu_limit() -> float | None:
    try:
        with open('/sys/fs/cgroup/cpu.max') as f:
            quota, period = f.read().split()[:2]
    except OSError:
        try:
            with open('/sys/fs/cgroup/cpu/cpu.cfs_quota_us') as f, open('/sys/fs/cgroup/cpu/cpu.cfs_period_us') as g:
                quota, period = f.read().strip(), g.read().strip()
        except OSError:
            return None
    return round(int(quota) / int(period), 2) if quota.lstrip('-').isdigit() and int(quota) > 0 else None


def limit_bytes() -> int | None:
    for path in ('/sys/fs/cgroup/memory.max', '/sys/fs/cgroup/memory/memory.limit_in_bytes'):
        try:
            with open(path) as f:
                said = f.read().strip()
        except OSError:
            continue
        if said.isdigit():
            return int(said)
    return None


def meminfo(key: str) -> int:
    with open('/proc/meminfo') as f:
        return next(int(line.split()[1]) * 1024 for line in f if line.startswith(key + ':'))


def cpu() -> str:
    with open('/proc/cpuinfo') as f:
        return next((line.split(':', 1)[1].strip() for line in f if line.startswith('model name')), '?')


def disk_gbps(total: int = GIB, chunk: int = 16 << 20) -> float:
    """Written, synced, dropped from the page cache, then read back: the probe method of suites/tensorfs/probe."""
    block = os.urandom(chunk)
    fd, path = tempfile.mkstemp(dir=sys.prefix)
    try:
        for _ in range(total // chunk):
            os.write(fd, block)
        os.fsync(fd)
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.lseek(fd, 0, os.SEEK_SET)
        start = time.perf_counter()
        while os.read(fd, chunk):
            pass
        return round(total / (time.perf_counter() - start) / 1e9, 2)
    finally:
        os.close(fd)
        os.unlink(path)


@app.entrypoint(accelerator=True)
def measure(payload: Calibrate) -> Measured:
    dev = torch.device(str(device()))
    g = rng.generator(payload.seed)

    def draw(*shape: int) -> torch.Tensor:
        return torch.randn(shape, dtype=torch.bfloat16, generator=g).to(dev)
    a, b = draw(8192, 8192), draw(8192, 8192)
    gemm = timed(lambda: a @ b, dev, batch=2)
    del a, b
    q, k, v = draw(1, 24, 4096, 128), draw(1, 24, 4096, 128), draw(1, 24, 4096, 128)
    attention = timed(lambda: F.scaled_dot_product_attention(q, k, v), dev)
    del q, k, v
    x = draw(1, 512, 128, 128).to(memory_format=torch.channels_last)
    w = draw(512, 512, 3, 3).to(memory_format=torch.channels_last)
    conv = timed(lambda: F.conv2d(x, w, padding=1), dev)
    del x, w
    host = torch.empty(GIB, dtype=torch.uint8).pin_memory()
    there = torch.empty(GIB, dtype=torch.uint8, device=dev)
    h2d = timed(lambda: there.copy_(host, non_blocking=True), dev, runs=5, batch=1)
    del host, there
    one = torch.zeros(1, device=dev)

    def launches() -> None:
        for _ in range(10_000):
            one.add_(1)
        torch.cuda.synchronize(dev)
    launch = wall(launches, runs=3)  # ms per 10k launches: microseconds per launch is a tenth of it
    del one
    python, tokenized = wall(python_work), wall(tokenize)
    total, limit = meminfo('MemTotal'), limit_bytes()
    return Measured(cpu=cpu(), cores=len(os.sched_getaffinity(0)), cpu_limit=cpu_limit(), python_ms=python,
                    launch_us=round(launch / 10, 3), tokenize_ms=tokenized, host_gib=round(total / GIB, 2),
                    limit_gib=round(limit / GIB, 2) if limit and limit < total else None, gemm_ms=gemm,
                    attention_ms=attention, conv_ms=conv, h2d_gbps=round(GIB / h2d / 1e6, 2), disk_gbps=disk_gbps())
