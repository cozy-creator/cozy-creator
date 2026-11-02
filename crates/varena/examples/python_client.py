"""Run with the P4 TensorFS SDK/wheel and a CUDA GPU; no Cozy daemon/model required."""

import ctypes as C
import json

from tensorfs.varena import api


def main() -> None:
    cuda = C.CDLL("libcuda.so.1")
    copy = cuda.cuMemcpyDtoH_v2
    copy.argtypes = [C.c_void_p, C.c_uint64, C.c_size_t]
    copy.restype = C.c_int
    # Establish the same primary context for caller-owned CUDA work.
    from cuda_driver import primary

    with primary(cuda), api.open(0) as gpu, gpu.region(bytes(range(64))) as region:
        with gpu.plan({"read": [region]}) as plan:
            for _ in range(2):
                with (
                    plan.phase("read", "u8/64/copy/eager"),
                    gpu.use([region]) as used,
                    gpu.alloc(4096),
                ):
                    result = (C.c_uint8 * 64)()
                    assert copy(result, used.ptr(), 64) == 0
                    assert bytes(result) == bytes(range(64))
                gpu.trim()
        print(json.dumps({"python_region_refill": "pass", "stats": gpu.stats()}))


if __name__ == "__main__":
    main()
