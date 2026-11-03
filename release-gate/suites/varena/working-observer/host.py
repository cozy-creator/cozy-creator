"""Actual current-profile allocator installation and snapshot, without a CUDA owner."""
import ctypes
import hashlib
import json
from pathlib import Path

import torch
from tensorfs import _ext
from tensorfs.varena import torch as vt


assert torch.__version__ == "2.15.0+cu132"
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
assert not torch.cuda.is_initialized()
# This is the genuine Runtime's CPU selection seam, not public device activation.
vt._bind_allocator()
assert not torch.cuda.is_initialized()
first = torch.cuda.memory_snapshot()
assert first == [], first  # No native device has been opened in this actual process.
vt._bind_allocator()
assert torch.cuda.memory_snapshot() == first


class Failure(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_int32), ("cuda", ctypes.c_int32), ("message", ctypes.c_char * 512)]


library = vt._library
library.varena_torch_install.argtypes = [ctypes.POINTER(Failure)]
library.varena_torch_install.restype = ctypes.c_int32
failure = Failure()
assert library.varena_torch_install(ctypes.byref(failure)) == 0
assert failure.kind == 3 and b"installation identity mismatch" in failure.message
assert torch.cuda.memory_snapshot() == first
assert not torch.cuda.is_initialized()

paths = [Path(_ext.__file__), Path(vt.__file__), Path(library._name)]
print(json.dumps({
    "passed": True,
    "torch": torch.__version__,
    "torch_git": torch.version.git_version,
    "cuda_initialized": torch.cuda.is_initialized(),
    "actual_segments": first,
    "repeated_runtime_binding_idempotent": True,
    "second_native_install_refused": bytes(failure.message).decode(errors="replace"),
    "files": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},
    "scope": "actual built extension and C++ allocator installation/snapshot only; no CUDA owner, trace delivery, NCCL collective or GPU lifetime proof",
}, indent=2))
