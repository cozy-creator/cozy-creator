"""Actual foreign Storage overlap refusal, including a strided read-only source."""

import hashlib
import json
from pathlib import Path

import torch
from cozy_runtime.internal.parallel import native_ulysses as native

backing = bytearray(256)
source = torch.frombuffer(backing, dtype=torch.float32)
other = torch.frombuffer(backing, dtype=torch.float32)
assert source.untyped_storage()._cdata != other.untyped_storage()._cdata
assert source.data_ptr() == other.data_ptr() and not torch._C._is_alias_of(
    source, other
)
cases = []
for value in (source, source[::2]):
    before = bytes(backing)
    try:
        native.validate_transport((value,), {"actual_destination": other})
        raise AssertionError("physical overlap accepted")
    except ValueError as error:
        assert "storage span" in str(error)
    assert bytes(backing) == before
    native.validate_transport((value,), {"actual_destination": torch.empty(64)})
    cases.append(
        {
            "contiguous": value.is_contiguous(),
            "physical_overlap_refused_before_writes": True,
        }
    )
assert (
    not torch.cuda.is_initialized()
    and "libcuda.so" not in Path("/proc/self/maps").read_text()
)
print(
    json.dumps(
        {
            "passed": True,
            "scope": "actual CPU frombuffer Storage spans; no CUDA proof",
            "runtime_source_sha256": hashlib.sha256(
                Path(native.__file__).read_bytes()
            ).hexdigest(),
            "cases": cases,
        },
        indent=2,
    )
)
