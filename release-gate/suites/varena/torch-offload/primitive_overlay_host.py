"""Real package/native771/provider host verification; no GPU stand-ins."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import importlib.util
import inspect
import json
from pathlib import Path

import tensorfs
import torch
from tensorfs import _ext
from tensorfs.varena import _offload_auto, _runtime, api
from tensorfs.varena import torch as vt


def run(args):
    assert not hasattr(_offload_auto, "_AutomaticGraphs")
    assert "graphs" not in inspect.signature(vt.offload).parameters
    assert _runtime.CacheOwner.capability() == 1
    assert hasattr(api.Use, "wait_ready")
    assert api.core_identity() == _ext._varena_core_identity()
    model = torch.nn.Sequential(torch.nn.Linear(4, 4), torch.nn.ReLU()).eval()
    value = torch.arange(8.0).reshape(2, 4)
    with torch.no_grad():
        expected = model(value)
    owner = vt.offload(model, device="cpu")
    with torch.no_grad():
        actual = model(value)
    torch.testing.assert_close(actual, expected, rtol=0, atol=0)
    assert not hasattr(owner, "_automatic")
    owner.close()
    spec = importlib.util.spec_from_file_location(
        "real_primitive_provider", args.provider_python
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    provider = module.Provider(args.provider)
    provider.host_contract()
    assert not torch.cuda.is_initialized()
    root = Path(tensorfs.__file__).parent
    return {
        "passed": True,
        "scope": "real package/provider/eager CPU only; GPU UNRUN",
        "controller_wired": False,
        "normal_offload": "eager",
        "version": importlib.metadata.version("tensorfs"),
        "torch": torch.__version__,
        "package": str(root),
        "shared_core_identity": True,
        "cache_protocol": 1,
        "native_core_sha256": hashlib.sha256(
            (root / ".native/libvarena.so").read_bytes()
        ).hexdigest(),
        "native_extension_sha256": hashlib.sha256(
            (root / "_ext.abi3.so").read_bytes()
        ).hexdigest(),
        "provider_host_contract": True,
        "cuda_initialized": False,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--provider-python", type=Path, required=True)
    options = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(options), indent=2))
