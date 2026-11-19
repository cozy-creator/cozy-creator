"""hostpin-probe.py <bytes>: PROOF ONLY (var-030 section 8 W8). What the kernel counts for <bytes> of
cuMemHostAlloc(PORTABLE | DEVICEMAP), the allocation varena's host fallback makes, on this pod's driver: this process's
cgroup charge at every level of its path (what varena's host admission reads at the tightest limit), the machine's
MemAvailable, and the process's own RssAnon, RssFile and RssShmem, each as the change from before it to while it is
held, then to after it is freed. One line. libcuda through ctypes, as suites/bench/vram.py."""
from __future__ import annotations

import ctypes
import os
import sys


def cgroups() -> list[str]:
    """This process's memory cgroup and each parent, innermost first (v2, else v1's memory controller)."""
    lines = open('/proc/self/cgroup').read().split('\n')
    v2 = [line[3:] for line in lines if line.startswith('0::')]
    v1 = [line.split(':', 2)[2] for line in lines if line.count(':') >= 2 and 'memory' in line.split(':', 2)[1].split(',')]
    if v2 and os.path.isfile('/sys/fs/cgroup/cgroup.controllers'):
        base, path = '/sys/fs/cgroup', v2[0]
    elif v1:
        base, path = '/sys/fs/cgroup/memory', v1[0]
        for row in open('/proc/self/mountinfo'):
            f = row.split(' ')
            dash = f.index('-') if '-' in f else -1
            if dash > 0 and f[dash + 1] == 'cgroup' and 'memory' in f[dash + 3].strip().split(','):
                base = f[4]
                if f[3] != '/' and path.startswith(f[3]):
                    path = path[len(f[3]):]
                break
    else:
        return []
    out, path = [], '/' + path.strip('/')
    while True:
        out.append(base + path.rstrip('/'))
        if path == '/':
            return out
        path = os.path.dirname(path)


def number(path: str) -> int | None:
    try:
        return int(open(path).read().strip())
    except (OSError, ValueError):
        return None


def stat(path: str, keys: tuple[str, ...]) -> dict[str, int]:
    try:
        rows = dict(line.split()[:2] for line in open(path) if line.strip())
    except OSError:
        return {}
    return {k: int(rows[n]) for k in keys for n in (k, 'total_' + k) if n in rows}


def reading(levels: list[str]) -> dict[str, int]:
    out: dict[str, int] = {}
    for line in open('/proc/meminfo'):
        if line.startswith('MemAvailable:'):
            out['MemAvailable'] = int(line.split()[1]) * 1024
    for line in open('/proc/self/status'):
        name = line.split(':')[0]
        if name in ('RssAnon', 'RssFile', 'RssShmem'):
            out[name] = int(line.split()[1]) * 1024
    for i, level in enumerate(levels):
        charge = number(level + '/memory.current')
        if charge is None:
            charge = number(level + '/memory.usage_in_bytes')
        if charge is not None:
            out[f'charge[{i}]'] = charge
        for key, value in stat(level + '/memory.stat', ('anon', 'file', 'kernel', 'shmem', 'rss', 'cache')).items():
            out[f'{key}[{i}]'] = value
    return out


def main(nbytes: int) -> None:
    cu = ctypes.CDLL('libcuda.so.1')
    cu.cuDeviceGet.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_int]
    cu.cuCtxCreate_v2.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_uint, ctypes.c_int]
    cu.cuMemHostAlloc.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_size_t, ctypes.c_uint]
    cu.cuMemFreeHost.argtypes = [ctypes.c_void_p]

    def ok(r: int, what: str) -> None:
        if r != 0:
            sys.exit(f'hostpin-probe: {what}: CUDA error {r}')

    dev, ctx, host = ctypes.c_int(), ctypes.c_void_p(), ctypes.c_void_p()
    ok(cu.cuInit(0), 'cuInit')
    ok(cu.cuDeviceGet(ctypes.byref(dev), 0), 'cuDeviceGet')
    ok(cu.cuCtxCreate_v2(ctypes.byref(ctx), 0, dev), 'cuCtxCreate')
    levels = cgroups()
    before = reading(levels)
    ok(cu.cuMemHostAlloc(ctypes.byref(host), nbytes, 0x01 | 0x02), 'cuMemHostAlloc')
    ctypes.memset(host, 1, nbytes)
    held = reading(levels)
    ok(cu.cuMemFreeHost(host), 'cuMemFreeHost')
    after = reading(levels)

    def moved(now: dict[str, int]) -> str:
        return ', '.join(f'{k} {(now[k] - before[k]) / 2**20:+.0f}' for k in before if k in now)

    print(f'hostpin-probe: {nbytes >> 20} MiB of cuMemHostAlloc, MiB moved while held: {moved(held)} | after it is freed: {moved(after)} '
          f'| cgroups innermost first: {" ".join(levels) or "none"}')


if __name__ == '__main__':
    main(int(sys.argv[1]))
