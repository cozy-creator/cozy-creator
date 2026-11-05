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
            region.set_priority("high")
            for number in range(2):
                with (
                    plan.phase("read", "u8/64/copy/eager"),
                    gpu.use([region], stream.handle) as used,
                ):
                    # The first pass runs on the estimate, the second on what the first measured.
                    levels = {"whole": 8192, "half": 4096}
                    level = gpu.split_select("copy/64", list(levels.items()), stream.handle)
                    assert level["measured"] == number, level
                    with plan.split("copy/64", level, stream.handle) as split, gpu.alloc(levels[level["id"]], stream.handle):
                        assert stream.read(used.ptr(), 64) == bytes(range(64))
                    assert (split.peak, split.counted) == (levels[level["id"]], 1), level
        print(json.dumps({"python_region_refill": "pass", "room": gpu.room(), "stats": gpu.stats()}))


if __name__ == "__main__":
    main()
