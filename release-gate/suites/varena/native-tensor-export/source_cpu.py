"""Real Torch CPU qualification for immutable host Sources; no CUDA initialization.

Run using an environment with Torch installed. This standalone host client imports
only the new Source module so it can qualify host snapshots before building a wheel.
It makes no claim about native producer ordering, CUDA exports or reclamation.
"""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import sys
from pathlib import Path

import torch

root = Path(__file__).resolve().parents[4]
path = root / "crates/tensorfs-py/python/tensorfs/varena/sources.py"
spec = importlib.util.spec_from_file_location("varena_source_host_client", path)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
Source = module.Source


def values(source):
    storage = torch.frombuffer(
        bytearray(source.data), dtype=getattr(torch, source.dtype)
    )
    return storage.as_strided(
        source.shape,
        source.strides,
        source.byte_offset // (module.dtype_descriptor(source.dtype)[1] // 8),
    )


checks = []

# Snapshot the whole storage behind an interior, transposed Tensor. Mutating the
# original after the snapshot must never change the immutable Source version.
original = torch.arange(24, dtype=torch.float32).reshape(4, 6)
view = original[1:4, 1:5].T
expected = view.clone()
source = Source.from_tensor(view)
assert source.nbytes == original.untyped_storage().nbytes()
assert source.shape == tuple(view.shape)
assert source.strides == tuple(view.stride())
assert source.byte_offset == view.storage_offset() * view.element_size()
original.add_(100)
assert torch.equal(values(source), expected)
checks.append("whole-storage interior transpose snapshot survives original mutation")

# Conversion changes the storage version, retaining all view descriptors and the
# original bytes. A second conversion exercises the VAE half/float shape of use.
old_bytes = source.data
half = source.derive(torch.float16)
restored = half.derive("float32")
assert source.data == old_bytes
assert half.dtype == "float16" and half.nbytes * 2 == source.nbytes
assert half.shape == source.shape and half.strides == source.strides
assert half.byte_offset * 2 == source.byte_offset
assert restored.byte_offset == source.byte_offset
assert torch.equal(values(half), expected.half())
assert torch.equal(values(restored), expected)
assert source.derive("float32") is source
checks.append("half/float storage versions preserve strides/offset and original bytes")

raw = bytearray([1, 2, 3, 4])
buffer_source = Source.from_buffer(memoryview(raw), dtype="uint8", shape=(2, 2))
raw[:] = bytes([9, 9, 9, 9])
assert buffer_source.data == bytes([1, 2, 3, 4])
try:
    buffer_source.data = b"changed"
except dataclasses.FrozenInstanceError:
    checks.append("mutable buffer snapshot and Source attribute immutability")
else:
    raise AssertionError("Source accepted mutation")

for name in (
    "bool",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "int8",
    "int16",
    "int32",
    "int64",
    "float16",
    "bfloat16",
    "float32",
    "float64",
    "complex64",
    "complex128",
):
    tensor = torch.tensor([0, 1, 2, 3], dtype=getattr(torch, name))
    scalar_source = Source.from_tensor(tensor)
    assert scalar_source.dtype == name
    assert torch.equal(values(scalar_source), tensor)
    converted = scalar_source.derive("float64")
    assert torch.equal(values(converted), tensor.to(torch.float64))
checks.append("all 15 admitted dtypes snapshot and derive through real Torch CPU")

scalar = Source.from_tensor(torch.tensor(4.5))
assert scalar.shape == () and scalar.strides == () and values(scalar).item() == 4.5
empty = Source.from_tensor(torch.empty((2, 0, 3)))
assert empty.nbytes == 0 and empty.shape == (2, 0, 3)
assert empty.derive("float16").data == b""
broadcast = Source.from_tensor(torch.tensor([3.0]).expand(2, 3))
assert broadcast.strides == (0, 0)
assert torch.equal(values(broadcast.derive("float16")), torch.full((2, 3), 3.0).half())
parameter = torch.nn.Parameter(torch.tensor([1.0, 2.0]))
assert torch.equal(values(Source.from_tensor(parameter)), parameter.detach())
checks.append("scalar/empty/broadcast/Parameter metadata and values")

refused = 0
for kwargs in (
    {"dtype": "float32", "shape": (2,)},
    {"dtype": "uint8", "shape": (-1,)},
    {"dtype": "uint8", "shape": (2,), "strides": ()},
    {"dtype": "uint8", "shape": (2,), "strides": (-1,)},
    {"dtype": "uint8", "shape": (2,), "strides": (4,)},
    {"dtype": "uint16", "shape": (1,), "byte_offset": 1},
    {"dtype": "float8_e4m3fn", "shape": (1,)},
):
    try:
        Source.from_buffer(b"1234", **kwargs)
    except ValueError:
        refused += 1
    else:
        raise AssertionError(f"invalid Source accepted: {kwargs}")
for tensor in (
    torch.empty(2, device="meta"),
    torch.sparse_coo_tensor([[0]], [1.0], (2,)),
    torch.tensor([1 + 2j]).conj(),
):
    try:
        Source.from_tensor(tensor)
    except ValueError:
        refused += 1
    else:
        raise AssertionError("unsupported Tensor source accepted")
try:
    Source.from_buffer(memoryview(bytearray(8))[::2], dtype="uint8", shape=(4,))
except ValueError:
    refused += 1
else:
    raise AssertionError("noncontiguous input buffer accepted")
checks.append(f"{refused} invalid dtype/view/storage inputs refused before native use")

assert not torch.cuda.is_initialized()
print(
    json.dumps(
        {"torch": torch.__version__, "cuda_initialized": False, "checks": checks}
    )
)
