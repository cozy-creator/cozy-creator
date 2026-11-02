"""Owner-selected interior holes, recommit and a shrink callback that may decline."""

import ctypes as C
import json
import threading

from cuda_driver import primary
from tensorfs import varena
from tensorfs.varena import api


def main() -> None:
    calls = []
    invoked = threading.Event()

    def shrink(cache, requested):
        # A scheduler with no preemptible sequences declines. Merely being a cache
        # does not make a live KV range available to the allocator.
        calls.append(requested)
        invoked.set()
        return False

    cuda = C.CDLL("libcuda.so.1")
    memset = cuda.cuMemsetD8_v2
    memset.argtypes = [C.c_uint64, C.c_ubyte, C.c_size_t]
    copy = cuda.cuMemcpyDtoH_v2
    copy.argtypes = [C.c_void_p, C.c_uint64, C.c_size_t]
    with primary(cuda), api.open() as gpu:
        cache = gpu.cache(64 << 20, shrink=shrink)
        page = cache.info()["granularity"]
        cache.commit(0, 3 * page)
        base = cache.info()["base"]
        assert memset(base, 17, 3 * page) == 0
        cache.release(page, page)
        assert cuda.cuCtxSynchronize() == 0
        assert cache.info()["live"] == 2 * page
        cache.commit(page, page)
        assert memset(base + page, 29, page) == 0
        for offset, value in [(0, 17), (page, 29), (2 * page, 17)]:
            result = (C.c_uint8 * page)()
            assert copy(result, base + offset, page) == 0
            assert bytes(result) == bytes([value]) * page
        # Explicit native request uses the same callback queue as allocation pressure.
        # This example proves declined shrink, not allocator-triggered recovery.
        request = varena.request_cache_shrink(0, page)
        assert request is not None
        assert invoked.wait(10), "shrink callback did not run"
        assert cache.info()["live"] == 3 * page
        cache.release(0, 3 * page)
        assert cuda.cuCtxSynchronize() == 0
        assert cache.info()["mapped"] == 0
        cache.close()
        print(
            json.dumps(
                {
                    "cache_holes": "pass",
                    "declined_requests": calls,
                    "stats": gpu.stats(),
                }
            )
        )


if __name__ == "__main__":
    main()
