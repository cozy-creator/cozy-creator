#!/usr/bin/env python3
"""A program that is no participant: raw driver allocations, as a game makes them, with a plain process
over `cozy machine ssh`. Takes free plus EXTRA in 256 MiB blocks, each retried every 50 ms, holds them,
then frees them. Prints each block's wait: the wait is how long idle executors took to give it back.
  foreign.py <extra GiB> <give up after s> <hold s>
"""
import ctypes
import json
import sys
import time

BLOCK = 256 << 20
EXTRA, GIVE_UP, HOLD = float(sys.argv[1]) * 2**30, float(sys.argv[2]), float(sys.argv[3])
cu = ctypes.CDLL("libcuda.so.1")
dev, ctx = ctypes.c_int(), ctypes.c_void_p()
assert cu.cuInit(0) == 0 and cu.cuDeviceGet(ctypes.byref(dev), 0) == 0
assert cu.cuDevicePrimaryCtxRetain(ctypes.byref(ctx), dev) == 0 and cu.cuCtxSetCurrent(ctx) == 0


def info() -> tuple[int, int]:
    free, total = ctypes.c_size_t(), ctypes.c_size_t()
    cu.cuMemGetInfo_v2(ctypes.byref(free), ctypes.byref(total))
    return free.value, total.value


free0, total = info()
reserve = max(total // 16, 512 << 20)
held: list[int] = []
waits: list[float] = []
began = time.monotonic()
for _ in range(int((free0 + EXTRA) // BLOCK)):
    start = time.monotonic()
    pointer: ctypes.c_uint64 | None = ctypes.c_uint64()
    while cu.cuMemAlloc_v2(ctypes.byref(pointer), ctypes.c_size_t(BLOCK)) != 0:  # type: ignore[arg-type]  # None only after the loop's break
        if time.monotonic() - start > GIVE_UP:
            pointer = None
            break
        time.sleep(0.05)
    if pointer is None:
        break
    held.append(pointer.value)
    waits.append(round(time.monotonic() - start, 3))
ramp = time.monotonic() - began
free_held: list[int] = []
until = time.monotonic() + HOLD
while time.monotonic() < until:
    free_held.append(info()[0])
    time.sleep(0.25)
print(json.dumps({
    "reserve_gib": reserve / 2**30, "total_gib": total / 2**30, "free_start_gib": free0 / 2**30,
    "blocks": len(held), "taken_gib": len(held) * BLOCK / 2**30, "gave_up": bool(waits) and len(held) < int((free0 + EXTRA) // BLOCK),
    "ramp_s": round(ramp, 2), "longest_wait_s": max(waits, default=0), "waited_blocks": sum(w > 0.01 for w in waits),
    "free_min_held_gib": min(free_held, default=0) / 2**30, "free_end_held_gib": (free_held or [0])[-1] / 2**30, "waits": waits}))
for address in held:
    cu.cuMemFree_v2(ctypes.c_uint64(address))
