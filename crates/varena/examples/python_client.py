"""Python reads a Region's exact bytes back. Needs the TensorFS wheel and a CUDA GPU, nothing else."""

import ctypes as C
import json

from cuda_driver import Stream, primary
from tensorfs import varena


def main() -> None:
    cuda = C.CDLL("libcuda.so.1")
    # Your own CUDA work uses the same primary context.
    with primary(cuda), Stream(cuda) as stream, varena.open(0) as gpu, gpu.region(bytes(range(64))) as region:
        with gpu.plan({"read": [region]}) as plan:
            for _ in range(2):
                with (
                    plan.phase("read", "u8/64/copy/eager"),
                    gpu.use([region], stream.handle) as used,
                    gpu.alloc(4096, stream.handle),
                ):
                    assert stream.read(used.ptr(), 64) == bytes(range(64))
        print(json.dumps({"python_region_refill": "pass", "stats": gpu.stats()}))


if __name__ == "__main__":
    main()
