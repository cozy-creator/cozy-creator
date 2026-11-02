"""Owner-selected interior holes, recommit and a shrink callback that may decline."""

import ctypes as C
import json
import threading

from cuda_driver import Stream, primary
from tensorfs import varena
from tensorfs.varena import _runtime


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
    with primary(cuda), Stream(cuda) as stream, varena.open() as gpu:
        cache = gpu.cache(64 << 20, shrink=shrink)
        page = cache.info()["granularity"]
        cache.commit(0, 3 * page)
        base = cache.info()["base"]
        stream.fill(base, 17, 3 * page)
        cache.release(page, page, stream.handle)
        stream.finish()
        assert cache.info()["live"] == 2 * page
        cache.commit(page, page)
        stream.fill(base + page, 29, page)
        for offset, value in [(0, 17), (page, 29), (2 * page, 17)]:
            assert stream.read(base + offset, page) == bytes([value]) * page
        # Private qualification trigger uses the same callback queue as allocation pressure.
        # This example proves declined shrink, not allocator-triggered recovery.
        request = _runtime.request_cache_shrink(0, page)
        assert request is not None
        assert invoked.wait(10), "shrink callback did not run"
        assert cache.info()["live"] == 3 * page
        cache.release(0, 3 * page, stream.handle)
        stream.finish()
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
