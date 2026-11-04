#!/usr/bin/env python3
"""Make a bigger card a smaller one, for real: a plain process holds all but <GiB> of it, so a driver
refusal is a driver refusal. `ballast.py <GiB left free>`. Holds until killed."""
import ctypes
import sys
import time

cu = ctypes.CDLL("libcuda.so.1")
dev, ctx = ctypes.c_int(), ctypes.c_void_p()
assert cu.cuInit(0) == 0 and cu.cuDeviceGet(ctypes.byref(dev), 0) == 0
assert cu.cuDevicePrimaryCtxRetain(ctypes.byref(ctx), dev) == 0 and cu.cuCtxSetCurrent(ctx) == 0
free, total = ctypes.c_size_t(), ctypes.c_size_t()
cu.cuMemGetInfo_v2(ctypes.byref(free), ctypes.byref(total))
held = free.value - int(float(sys.argv[1]) * 2**30)
pointer = ctypes.c_uint64()
assert held <= 0 or cu.cuMemAlloc_v2(ctypes.byref(pointer), ctypes.c_size_t(held)) == 0
cu.cuMemGetInfo_v2(ctypes.byref(free), ctypes.byref(total))
print(f"holding {held / 2**30:.1f} GiB; {free.value / 2**30:.1f} of {total.value / 2**30:.1f} GiB free", flush=True)
while True:
    time.sleep(3600)
