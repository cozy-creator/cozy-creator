"""vram.py <free bytes> <ready file>: a card of that size for every other process (pod.sh vram). Holds GPU 0's memory until
<free bytes> less what other processes already hold is free, then writes '<pid> <held> <free> <total> <others>' (bytes) to
<ready file> and sleeps until killed. <held> is all the holder takes from the card, its own CUDA context included
(total less free less others): the driver's peak during a capped run less <held> is what that run used. libcuda through ctypes: no torch on the pod's python needed."""
import ctypes
import os
import subprocess
import sys
import time

OOM = 2  # CUDA_ERROR_OUT_OF_MEMORY


def others() -> int:
    """What other processes hold on GPU 0 now (0 where the container can't see them)."""
    out = subprocess.run(['nvidia-smi', '-i', '0', '--query-compute-apps=used_memory', '--format=csv,noheader,nounits'],
                         capture_output=True, text=True).stdout
    return sum(int(x) for x in out.split() if x.isdigit()) << 20


def main(free_want: int, ready: str) -> None:
    held_by_others = others()
    cu = ctypes.CDLL('libcuda.so.1')
    cu.cuDeviceGet.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_int]
    cu.cuCtxCreate_v2.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_uint, ctypes.c_int]
    cu.cuMemGetInfo_v2.argtypes = [ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
    cu.cuMemAlloc_v2.argtypes = [ctypes.POINTER(ctypes.c_uint64), ctypes.c_size_t]

    def ok(r: int, what: str) -> None:
        if r != 0:
            sys.exit(f'{what}: CUDA error {r}')

    dev, ctx = ctypes.c_int(), ctypes.c_void_p()
    ok(cu.cuInit(0), 'cuInit')
    ok(cu.cuDeviceGet(ctypes.byref(dev), 0), 'cuDeviceGet')
    ok(cu.cuCtxCreate_v2(ctypes.byref(ctx), 0, dev), 'cuCtxCreate')
    free, total = ctypes.c_size_t(), ctypes.c_size_t()
    target = max(free_want - held_by_others, 0)
    chunk, ptrs = 1 << 30, []
    while chunk >= 8 << 20:
        ok(cu.cuMemGetInfo_v2(ctypes.byref(free), ctypes.byref(total)), 'cuMemGetInfo')
        want = free.value - target
        if want < 8 << 20:
            break
        n = min(chunk, want) >> 21 << 21 or want
        p = ctypes.c_uint64()
        r = cu.cuMemAlloc_v2(ctypes.byref(p), n)
        if r == OOM:
            chunk //= 2
            continue
        ok(r, 'cuMemAlloc')
        ptrs.append(p)
    ok(cu.cuMemGetInfo_v2(ctypes.byref(free), ctypes.byref(total)), 'cuMemGetInfo')
    with open(ready + '.tmp', 'w') as f:
        f.write(f'{os.getpid()} {total.value - free.value - held_by_others} {free.value} {total.value} {held_by_others}')
    os.replace(ready + '.tmp', ready)
    while True:
        time.sleep(3600)


if __name__ == '__main__':
    main(int(sys.argv[1]), sys.argv[2])
