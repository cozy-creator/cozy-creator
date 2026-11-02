"""Explicit nonblocking stream ownership for the real mixed-client GPU proofs."""

import ctypes as C
from contextlib import contextmanager


@contextmanager
def owned_stream(torch, vt, device=0):
    cuda = C.CDLL("libcuda.so.1")
    create, destroy = cuda.cuStreamCreate, cuda.cuStreamDestroy_v2
    create.argtypes, create.restype = [C.POINTER(C.c_void_p), C.c_uint], C.c_int
    destroy.argtypes, destroy.restype = [C.c_void_p], C.c_int
    pointer = C.c_void_p()
    assert create(C.byref(pointer), 1) == 0  # CU_STREAM_NON_BLOCKING
    stream = torch.cuda.ExternalStream(pointer.value, device=device)
    try:
        yield stream
    finally:
        # Complete only this caller's submissions. The caller must drain its checked
        # or deferred native owners before leaving this scope.
        done = torch.cuda.Event()
        done.record(stream)
        done.synchronize()
        vt.retire_stream(stream, device)
        assert destroy(pointer) == 0
