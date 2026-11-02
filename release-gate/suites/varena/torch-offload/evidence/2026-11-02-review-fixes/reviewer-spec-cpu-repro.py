"""Read-only CPU reproduction of the frozen adapter's two source defects."""

import hashlib
import json
from pathlib import Path

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload

root = Path("/home/fidika/cozy/.worktrees/cozy-creator/xs138-p4-torch-offload")
source = root / "crates/tensorfs-py/python/tensorfs/varena/_offload.py"
loaded = Path(_offload.__file__)
assert source.read_bytes() == loaded.read_bytes()

model = torch.nn.Linear(3, 2, bias=False).eval()
owner = vt.offload(model, device="cpu")
out = torch.empty_like(model.weight)
assert type(out) is torch.Tensor
try:
    torch.add(model.weight, 1, out=out)
except RuntimeError as error:
    out_error = str(error)
else:
    raise AssertionError("out= failure no longer reproduces")
assert "meta to device cpu" in out_error


def complex_model():
    module = torch.nn.Module().eval()
    module.p = torch.nn.Parameter(torch.tensor([1 + 2j], dtype=torch.complex64))
    module.register_buffer("b", torch.tensor([3 + 4j], dtype=torch.complex64))
    return module


stock = complex_model().float()
adopted = complex_model()
complex_owner = vt.offload(adopted, device="cpu")
adopted.float()
assert stock.p.dtype == stock.b.dtype == torch.complex64
assert stock.p.tolist() == [1 + 2j] and stock.b.tolist() == [3 + 4j]
assert adopted.p.dtype == adopted.b.dtype == torch.float32
assert adopted.p.clone().tolist() == [1.0] and adopted.b.tolist() == [3.0]
assert not torch.cuda.is_initialized()

print(json.dumps({
    "reviewed_commit": "f574e526a7dfb528881e02315dc99bdc194b8479",
    "torch": torch.__version__,
    "adapter_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
    "loaded_adapter": str(loaded),
    "loaded_matches_frozen_source": True,
    "out_independent_tensor": {"status": "reproduced", "error": out_error},
    "complex_float": {
        "status": "reproduced",
        "stock": {"dtype": str(stock.p.dtype), "parameter": str(stock.p.tolist()), "buffer": str(stock.b.tolist())},
        "offloaded": {"dtype": str(adopted.p.dtype), "parameter": adopted.p.clone().tolist(), "buffer": adopted.b.tolist()},
    },
    "cuda_initialized": torch.cuda.is_initialized(),
}, indent=2))
