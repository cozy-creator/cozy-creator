"""Gate hook, appended to a package copy: every use of a scratch pool finds its pages gone and noise
in the ones that come back. A scratch pool promises nothing crosses its uses; if something does, the
output changes. cell.sh compares it with the same request on the same package without this hook."""

import ctypes
import os
import time

import tensorfs.varena.torch as vt
import torch

_CUDA = ctypes.CDLL("libcuda.so.1")
_CUDA.cuMemsetD8_v2.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t]
_prepare, _declare = vt.prepare_pool, vt.pool_scratch
_scratch: set[tuple[int, int]] = set()
_uses = [0, 0]  # uses scrambled, of which unmapped first
_COUNTS = os.environ.get("TMPDIR", "/tmp") + "/pool-scramble.count"


def pool_scratch(pool, held=(), device=0):
    _declare(pool, held, device)
    _scratch.add(tuple(pool))


def prepare_pool(pool, bytes, device=0):
    if bytes or tuple(pool) not in _scratch:
        return _prepare(pool, bytes, device)
    torch.cuda.synchronize(device)
    deadline = time.monotonic() + 2  # The end-of-use fence is sealed, then seen done, by a later poll.
    while not (gone := vt.give_back_idle_pools(device)) and time.monotonic() < deadline:
        time.sleep(0.001)
    got = _prepare(pool, 0, device)
    for segment in torch.cuda.memory_snapshot(mempool_id=tuple(pool)):
        code = _CUDA.cuMemsetD8_v2(segment["address"], 0xA5, segment["total_size"])
        assert code == 0, code
    _uses[0] += 1
    _uses[1] += bool(gone)
    with open(_COUNTS, "w") as counts:
        counts.write(f"{_uses[0]} {_uses[1]}\n")
    return got


vt.prepare_pool, vt.pool_scratch = prepare_pool, pool_scratch
