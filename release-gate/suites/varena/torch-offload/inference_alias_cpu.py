"""Actual immutable inference aliases, including the reproduced26475 detach failure."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import _Weight

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


class Model(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.arange(24.0).reshape(4, 6)[1:3, 1:5].T)
        self.tied = self.weight
        self.bias = torch.nn.Parameter(torch.arange(4.0))

    def forward(self, value):
        return torch.nn.functional.linear(value, self.weight, self.bias)


model = Model().eval()
reference = model.weight.detach().clone()
identity = id(model.weight)
owner = vt.offload(model, device="cpu")
operations = {
    "detach": lambda value: value.detach(),
    "repeated_detach": lambda value: value.detach().detach(),
    "slice": lambda value: value[:2, :],
    "transpose": lambda value: value.T,
    "narrow": lambda value: value.narrow(0, 1, 2),
    "view_same_shape": lambda value: value.view(4, 2),
    "data": lambda value: value.data,
    "split": lambda value: value.split(2),
}
results, escaped = [], []
for mode, context in (
    ("no_grad", torch.no_grad),
    ("inference_mode", torch.inference_mode),
):
    for name, operation in operations.items():
        expected = operation(reference)
        try:
            with context():
                actual = operation(model.weight)
                actuals = actual if isinstance(actual, tuple) else (actual,)
                controls = expected if isinstance(expected, tuple) else (expected,)
                for alias, control in zip(actuals, controls, strict=True):
                    assert isinstance(alias, _Weight)
                    assert alias._version_owner is model.weight._version_owner
                    assert alias.shape == control.shape
                    copy = alias.clone()
                    torch.testing.assert_close(copy, control, rtol=0, atol=0)
                    assert torch.is_inference(copy) == (mode == "inference_mode")
                    escaped.append((alias, control.clone()))
            results.append({"mode": mode, "operation": name, "pass": True})
        except RuntimeError as error:
            results.append(
                {"mode": mode, "operation": name, "pass": False, "error": str(error)}
            )
with torch.inference_mode():
    torch.testing.assert_close(
        model(torch.ones(1, 2)),
        torch.nn.functional.linear(torch.ones(1, 2), reference, torch.arange(4.0)),
        rtol=0,
        atol=0,
    )
for alias, expected in escaped:
    torch.testing.assert_close(alias.clone(), expected, rtol=0, atol=0)
    try:
        alias.zero_()
    except RuntimeError as error:
        assert "immutable" in str(error)
    else:
        raise AssertionError("escaped managed alias became mutable")
model.double()
assert model.weight is model.tied and id(model.weight) == identity
assert model.weight.dtype == torch.float64
for alias, expected in escaped:
    assert alias.dtype == torch.float32
    torch.testing.assert_close(alias.clone(), expected, rtol=0, atol=0)
owner.close()
for alias, expected in escaped:
    torch.testing.assert_close(alias.clone(), expected, rtol=0, atol=0)
with torch.inference_mode():
    born_inside = Model().eval()
    inside_owner = vt.offload(born_inside, device="cpu")
    inside_alias = born_inside.weight.detach()
    inside_values = inside_alias.clone()
    born_inside.double()
    assert born_inside.weight.dtype == torch.float64
    assert inside_alias.dtype == torch.float32
    torch.testing.assert_close(inside_alias.clone(), inside_values, rtol=0, atol=0)
    inside_owner.close()
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "results": results,
            "computation_preserves_caller_inference_mode": True,
            "tied_identity_and_escaped_versions": True,
        }
    ),
    flush=True,
)
assert all(row["pass"] for row in results)
