"""Fresh-child real observer refusal: retain the exact physical mapping, then exit."""
import argparse
import ctypes
import gc
import hashlib
import json
import os
from pathlib import Path

import torch
from tensorfs import _ext
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--probe", type=Path, required=True)
parser.add_argument("--probe-sha256", required=True)
parser.add_argument("--gpu-uuid", required=True)
parser.add_argument("--extension-sha256", required=True)
parser.add_argument("--core-sha256", required=True)
parser.add_argument("--shim-sha256", required=True)
parser.add_argument("--torch-adapter-sha256", required=True)
args = parser.parse_args()
assert hashlib.sha256(args.probe.read_bytes()).hexdigest() == args.probe_sha256
assert args.gpu_uuid != "UNBOUND"
assert torch.__version__ == "2.15.0+cu132" and not torch.cuda.is_initialized()
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
core_paths = {line.split()[-1] for line in Path("/proc/self/maps").read_text().splitlines()
              if line.split()[-1].endswith("/libvarena.so")}
assert len(core_paths) == 1, core_paths
files = {}
for name, path, expected in (
    ("core", next(iter(core_paths)), args.core_sha256),
    ("extension", _ext.__file__, args.extension_sha256),
    ("shim", Path(_ext.__file__).parent / "_varena_torch.so", args.shim_sha256),
    ("adapter", vt.__file__, args.torch_adapter_sha256),
):
    actual = hashlib.sha256(Path(path).read_bytes()).hexdigest()
    assert actual == expected, (name, actual, expected)
    files[name] = {"path": str(path), "sha256": actual}
vt.install(0)
uuid = str(torch.cuda.get_device_properties(0).uuid)
normalize = lambda x: x.removeprefix("GPU-").replace("-", "").lower()
assert normalize(uuid) == normalize(args.gpu_uuid)
# Torch's trace listeners cannot detach; retain this exact fixture code until exit.
probe = ctypes.CDLL(str(args.probe.resolve()), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
probe.observer_probe_attach.restype = ctypes.c_int
probe.observer_probe_reject.argtypes = [ctypes.c_uint64]
probe.observer_probe_count.argtypes = [ctypes.c_int]
probe.observer_probe_count.restype = ctypes.c_uint64
probe.observer_probe_read.argtypes = [ctypes.c_uint64, ctypes.c_size_t, ctypes.c_uint8]
probe.observer_probe_read.restype = ctypes.c_int
assert probe.observer_probe_attach() == 0
stream = torch.cuda.Stream()
with torch.cuda.stream(stream):
    value = torch.full((3 << 20,), 77, dtype=torch.uint8, device="cuda")
    stream.synchronize()
    pointer, bytes_ = value.data_ptr(), value.numel()
    assert probe.observer_probe_count(0) > 0
    segments = torch.cuda.memory_snapshot()
    segment = next(s for s in segments if s["address"] <= pointer < s["address"] + s["total_size"])
    assert pointer + bytes_ <= segment["address"] + segment["total_size"]
    probe.observer_probe_reject(segment["address"])
    del value
    gc.collect()
    stream.synchronize()
    try:
        vt.trim(0)
    except RuntimeError as error:
        refusal = str(error)
        assert "owned Working observer refusal" in refusal
    else:
        raise AssertionError("actual physical retirement did not reach the refusing observer")
    assert probe.observer_probe_count(2) == 1
    # Checked reads of this exact retained mapping make an accidental unmap a real red.
    # The child never reuses or claims reclamation of this poisoned allocation.
    assert probe.observer_probe_read(pointer, bytes_, 77) == 0
    try:
        torch.cuda.memory_snapshot()
    except RuntimeError as error:
        assert "working retirement observer" in str(error)
        poisoned = True
    else:
        raise AssertionError("refusing observer did not poison Working")
print(json.dumps({"passed": True, "gpu_uuid": uuid, "pid": os.getpid(), "files": files,
                  "target_segment": segment, "retained_readback_bytes": bytes_,
                  "map_notifications": probe.observer_probe_count(0),
                  "retirement_notifications": probe.observer_probe_count(1),
                  "target_refusals": probe.observer_probe_count(2), "poisoned": poisoned,
                  "refusal": refusal, "cleanup": "physical mapping retained until fresh-child process exit; no graceful release claim"}, indent=2))
