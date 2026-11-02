"""Run with the P4 TensorFS SDK/wheel and a CUDA GPU; no Cozy daemon/model required."""

import ctypes as C
import json

from tensorfs import varena


def main() -> None:
    cuda = C.CDLL("libcuda.so.1")
    # Establish the same primary context for caller-owned CUDA work.
    from cuda_driver import Stream, primary

    with primary(cuda), Stream(cuda) as stream, varena.open(0) as gpu, gpu.region(bytes(range(64))) as region:
        with gpu.plan({"read": [region]}) as plan:
            for _ in range(2):
                with (
                    plan.phase("read", "u8/64/copy/eager"),
                    gpu.use([region], stream.handle) as used,
                    gpu.alloc(4096, stream.handle),
                ):
                    assert stream.read(used.ptr(), 64) == bytes(range(64))
                gpu.trim()
        print(json.dumps({"python_region_refill": "pass", "stats": gpu.stats()}))


if __name__ == "__main__":
    main()
