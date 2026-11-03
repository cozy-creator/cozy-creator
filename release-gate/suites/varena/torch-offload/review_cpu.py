"""Actual public CPU regressions from the f574 review, compared with stock Torch."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs.varena import torch as vt

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
results = []


def case(name, run):
    try:
        run()
    except (AssertionError, RuntimeError, ValueError, TypeError) as error:
        results.append({"name": name, "pass": False, "error": str(error)})
    else:
        results.append({"name": name, "pass": True})


def output_buffers():
    model = torch.nn.Linear(3, 2, bias=False).eval()
    expected = model.weight.detach().clone()
    owner = vt.offload(model, device="cpu")
    out = torch.empty_like(model.weight)
    assert type(out) is torch.Tensor
    assert torch.add(model.weight, 1, out=out) is out
    torch.testing.assert_close(out, expected + 1)
    assert out.add_(model.weight) is out
    torch.testing.assert_close(out, expected * 2 + 1)
    values = torch.empty_like(model.weight)
    indices = torch.empty_like(model.weight, dtype=torch.int64)
    actual = torch.sort(model.weight, out=(values, indices))
    control = torch.sort(expected)
    assert actual.values is values and actual.indices is indices
    assert torch.equal(values, control.values) and torch.equal(indices, control.indices)
    out.zero_()
    assert torch.equal(model.weight.clone(), expected)
    owner.close()


class Mixed(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.p = torch.nn.Parameter(torch.tensor([1 + 2j], dtype=torch.complex64))
        self.tied = self.p
        self.real = torch.nn.Parameter(torch.tensor([2.5], dtype=torch.float64))
        self.register_buffer("b", torch.tensor([3 + 4j], dtype=torch.complex64))
        self.register_buffer("real_buffer", torch.tensor([1.25], dtype=torch.float64))

    def forward(self, x):
        return self.p * x + self.b + self.real + self.real_buffer


def conversion(method=None, dtype=None):
    stock, adopted = Mixed().eval(), Mixed().eval()
    parameter = adopted.p
    owner = vt.offload(adopted, device="cpu")
    old = parameter.detach()
    before = old.clone()
    if method is not None:
        getattr(stock, method)()
        getattr(adopted, method)()
    else:
        stock.to(dtype=dtype)
        adopted.to(dtype=dtype)
    assert adopted.p is adopted.tied is parameter
    assert torch.equal(old.clone(), before) and old.dtype == torch.complex64
    for name in ("p", "tied", "real", "b", "real_buffer"):
        control, actual = getattr(stock, name), getattr(adopted, name)
        assert control.dtype == actual.dtype, (name, control.dtype, actual.dtype)
        torch.testing.assert_close(actual.clone(), control.detach())
    with torch.no_grad():
        x = torch.ones(1)
        torch.testing.assert_close(adopted(x), stock(x))
    owner.close()


case(
    "ordinary out=, in-place working output and multiple writable outputs",
    output_buffers,
)
for method in ("float", "half", "double", "bfloat16"):
    case(
        method + " keeps complex weights/buffers",
        lambda method=method: conversion(method),
    )
case(
    "explicit to(float32) still casts complex weights/buffers",
    lambda: conversion(dtype=torch.float32),
)
case(
    "explicit to(complex128) converts floating and complex weights/buffers",
    lambda: conversion(dtype=torch.complex128),
)
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps({"torch": torch.__version__, "cuda": False, "results": results}),
    flush=True,
)
assert all(row["pass"] for row in results), "review regressions failed"
