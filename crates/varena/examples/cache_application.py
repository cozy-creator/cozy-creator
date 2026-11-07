"""A KV-style Cache: owner-chosen interior holes, recommit, and a shrink callback."""

import ctypes as C
import json

from cuda_driver import Stream, primary
from tensorfs import varena


def main() -> None:
    asked = []

    def shrink(cache, requested):
        # The core calls this under memory pressure. A scheduler with no preemptible sequences
        # declines: being a cache does not make a live KV range free memory.
        asked.append(requested)
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
        # More than the card has: the core asks this Cache, which declines and keeps every page.
        try:
            gpu.alloc(2 * gpu.room()["capacity"], stream.handle)
        except varena.Exhausted:
            pass
        else:
            raise AssertionError("more than the card was allocated")
        assert asked, "the core never asked the owner"
        assert cache.info()["live"] == 3 * page
        for offset, value in [(0, 17), (page, 29), (2 * page, 17)]:
            assert stream.read(base + offset, page) == bytes([value]) * page
        cache.release(0, 3 * page, stream.handle)
        stream.finish()
        assert cache.info()["mapped"] == 0
        cache.close()
        print(
            json.dumps(
                {
                    "cache_holes": "pass",
                    "declined_asks": len(asked),
                    "stats": gpu.stats(),
                }
            )
        )


if __name__ == "__main__":
    main()
