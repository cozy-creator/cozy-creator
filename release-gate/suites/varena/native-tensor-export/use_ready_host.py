"""Direct built-extension C ABI refusal check; no CUDA or Torch is required.

This checks host dispatch only. It does not qualify packaging, GPU completion,
resource retirement, capture admission or concurrent Use ownership.
"""

from __future__ import annotations

import argparse
import ctypes as C
import json
from pathlib import Path


def cuda_mappings() -> list[str]:
    return [
        line
        for line in Path("/proc/self/maps").read_text().splitlines()
        if "libcuda.so" in line or "libcudart.so" in line
    ]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--extension", type=Path, default=Path(".task/target/debug/lib_ext.so")
    )
    args = parser.parse_args()
    assert not cuda_mappings(), "host proof already has CUDA loaded"
    extension = args.extension.resolve(strict=True)
    native = C.CDLL(str(extension))
    wait = native.vr_v1_use_wait_ready
    wait.argtypes, wait.restype = [C.c_uint64], C.c_int32
    error = native.vr_v1_last_error
    error.argtypes, error.restype = [], C.c_char_p
    observations = []
    for handle in (0, 2**64 - 1):
        status = wait(handle)
        message = (error() or b"").decode("utf-8", "replace")
        assert status == 5, (handle, status, message)
        observations.append({"handle": handle, "status": status, "error": message})
    assert not cuda_mappings(), "invalid Use dispatch loaded CUDA"
    print(
        json.dumps(
            {
                "passed": True,
                "extension": str(extension),
                "invalid_handles": observations,
                "cuda_loaded": False,
                "gpu_qualified": False,
            }
        )
    )


if __name__ == "__main__":
    main()
