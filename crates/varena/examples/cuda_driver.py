"""Minimal CUDA context ownership for the standalone examples; no allocator."""

import ctypes as C
from contextlib import contextmanager


@contextmanager
def primary(cuda, device=0):
    retain = cuda.cuDevicePrimaryCtxRetain
    retain.argtypes = [C.POINTER(C.c_void_p), C.c_int]
    push = cuda.cuCtxPushCurrent_v2
    push.argtypes = [C.c_void_p]
    pop = cuda.cuCtxPopCurrent_v2
    pop.argtypes = [C.POINTER(C.c_void_p)]
    release = cuda.cuDevicePrimaryCtxRelease_v2
    release.argtypes = [C.c_int]
    context = C.c_void_p()
    assert cuda.cuInit(0) == 0
    assert retain(C.byref(context), device) == 0
    assert push(context) == 0
    try:
        yield
    finally:
        assert pop(C.byref(context)) == 0
        assert release(device) == 0
