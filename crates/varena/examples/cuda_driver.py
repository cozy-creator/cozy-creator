"""Caller-owned CUDA context, nonblocking stream and pinned readback for examples."""

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


class Stream:
    """Keep this owner until all application and Varena consumers have retired."""

    def __init__(self, cuda):
        self.cuda = cuda
        pointer = C.POINTER(C.c_void_p)
        for name, arguments in {
            "cuStreamCreate": [pointer, C.c_uint],
            "cuStreamDestroy_v2": [C.c_void_p],
            "cuEventCreate": [pointer, C.c_uint],
            "cuEventRecord": [C.c_void_p, C.c_void_p],
            "cuEventSynchronize": [C.c_void_p],
            "cuEventDestroy_v2": [C.c_void_p],
            "cuMemAllocHost_v2": [pointer, C.c_size_t],
            "cuMemFreeHost": [C.c_void_p],
            "cuMemsetD8Async": [C.c_uint64, C.c_ubyte, C.c_size_t, C.c_void_p],
            "cuMemcpyDtoHAsync_v2": [C.c_void_p, C.c_uint64, C.c_size_t, C.c_void_p],
        }.items():
            function = getattr(cuda, name)
            function.argtypes, function.restype = arguments, C.c_int
        self._stream, self._done = C.c_void_p(), C.c_void_p()
        assert cuda.cuEventCreate(C.byref(self._done), 2) == 0  # DISABLE_TIMING
        try:
            assert cuda.cuStreamCreate(C.byref(self._stream), 1) == 0  # NON_BLOCKING
        except BaseException:
            assert cuda.cuEventDestroy_v2(self._done) == 0
            raise

    @property
    def handle(self):
        return self._stream.value

    def finish(self):
        assert self.cuda.cuEventRecord(self._done, self._stream) == 0
        assert self.cuda.cuEventSynchronize(self._done) == 0

    def fill(self, address, value, length):
        assert self.cuda.cuMemsetD8Async(address, value, length, self._stream) == 0

    def read(self, address, length):
        host = C.c_void_p()
        assert self.cuda.cuMemAllocHost_v2(C.byref(host), length) == 0
        # On a failed submission/completion, leave the owning host allocation intact;
        # a failed driver operation is not permission to free an unproven borrower.
        assert self.cuda.cuMemcpyDtoHAsync_v2(host, address, length, self._stream) == 0
        self.finish()
        result = C.string_at(host, length)
        assert self.cuda.cuMemFreeHost(host) == 0
        return result

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.finish()
        assert self.cuda.cuEventDestroy_v2(self._done) == 0
        assert self.cuda.cuStreamDestroy_v2(self._stream) == 0
