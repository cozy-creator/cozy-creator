"""Exercise the built production bridge's CPU metadata/refusal boundaries.

No CUDA is initialized or required. This is a direct extension host check; it
makes no installed-wheel, native Pin, stream execution or GPU qualification claim.
"""

from __future__ import annotations

import argparse
import ctypes
import importlib.util
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--extension", required=True, type=Path)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location("_ext", args.extension.resolve())
ext = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ext)

native = ctypes.CDLL(str(args.extension.resolve()))
native.vr_v1_core_identity.restype = ctypes.c_uint64
assert native.vr_v1_core_identity() == ext._varena_core_identity()
export = ext._varena_public_use_dlpack
try:
    ext._varena_public_gpu_event_counts(0)
except RuntimeError as error:
    assert "invalid public Gpu handle" in str(error), str(error)
else:
    raise AssertionError("read-only native event observer accepted a missing Gpu")

# Actual production metadata validation must reject these before touching a Use,
# allocator or CUDA. The handle is deliberately absent, not a mocked owner.
invalid = [
    ((3, 32, [1], None, 0), "unsupported DLPack dtype"),
    ((2, 8, [1], None, 0), "unsupported DLPack dtype"),
    ((2, 32, [-1], None, 0), "tensor shape must be nonnegative"),
    ((2, 32, [2], [], 0), "tensor shape and stride ranks differ"),
    ((2, 32, [2], [-1], 0), "negative tensor strides are unsupported"),
    ((2, 32, [2**62, 8], None, 0), "tensor element count overflow"),
    ((2, 32, [2**62], [8], 0), "tensor stride span overflow"),
    ((2, 32, [2**62], None, 0), "tensor byte span overflow"),
    ((2, 32, [1], None, 1), "tensor byte offset is not aligned"),
    ((2, 32, [2], None, 2**64 - 4), "tensor byte offset overflow"),
]
for (code, bits, shape, strides, offset), message in invalid:
    try:
        export(0, 0, code, bits, shape, strides, offset, 3)
    except ValueError as error:
        assert message in str(error), str(error)
    else:
        raise AssertionError(f"accepted invalid native metadata: {message}")

# These valid descriptors reach the real installation guard rather than silently
# exporting with the stock allocator, which would miss Region record_stream.
valid = [
    (2, 32, [], None, 0),
    (2, 32, [0, 3], [3, 1], 0),
    (2, 32, [2, 3], [0, 0], 4),
    (4, 16, [2], [1], 0),
    (5, 128, [2], None, 0),
    (6, 8, [2], None, 0),
]
for code, bits, shape, strides, offset in valid:
    try:
        export(0, 0, code, bits, shape, strides, offset, 3)
    except ValueError as error:
        assert "install tensorfs.varena.torch" in str(error), str(error)
    else:
        raise AssertionError("export admitted storage without consumer routing")

print(
    json.dumps(
        {
            "native_bridge": "pass",
            "same_core": True,
            "invalid_layouts_refused": len(invalid),
            "valid_layouts_require_torch_install": len(valid),
            "cuda_execution": False,
            "native_event_observer_refuses_missing_gpu": True,
        }
    )
)
