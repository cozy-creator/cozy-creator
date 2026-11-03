"""Actual Torch trace-listener registration on the bound current allocator; no GPU."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path

import torch
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--probe", type=Path, required=True)
parser.add_argument("--probe-sha256", required=True)
args = parser.parse_args()
assert torch.__version__ == "2.15.0+cu132" and not torch.cuda.is_initialized()
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
assert hashlib.sha256(args.probe.read_bytes()).hexdigest() == args.probe_sha256
vt._bind_allocator()
probe = ctypes.CDLL(str(args.probe.resolve()), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
probe.observer_probe_attach.restype = ctypes.c_int
probe.observer_probe_count.argtypes = [ctypes.c_int]
probe.observer_probe_count.restype = ctypes.c_uint64
assert probe.observer_probe_attach() == 0
assert [probe.observer_probe_count(i) for i in range(3)] == [0, 0, 0]
assert torch.cuda.memory_snapshot() == []
assert not torch.cuda.is_initialized()
print(json.dumps({"passed": True, "cuda_initialized": False,
                  "scope": "actual installed current allocator accepts a real C++ trace listener; no callbacks were synthetically invoked and no GPU events are qualified",
                  "probe_sha256": args.probe_sha256}))
