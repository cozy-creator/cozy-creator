"""Actual CPU Conv3d, packing and declaration client; no CUDA/provider substitute."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from itertools import product
from pathlib import Path
import sys

import torch
from torch.utils._python_dispatch import TorchDispatchMode

parser = argparse.ArgumentParser()
parser.add_argument("--consumer", type=Path, required=True)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location("post_quant_consumer", args.consumer)
assert spec is not None and spec.loader is not None
consumer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = consumer
spec.loader.exec_module(consumer)
torch.set_num_threads(1)
torch.manual_seed(1731)


class Observe(TorchDispatchMode):
    def __init__(self):
        self.calls = []

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        self.calls.append(str(func))
        return func(*args, **(kwargs or {}))


rows = []
with torch.no_grad():
    for geometry, batch, input_dtype, weight_dtype in product(
        ((2, 3, 4), (7, 16, 16)), (1, 3, 7),
        (torch.float16, torch.float32), (torch.float16, torch.float32),
    ):
        module = torch.nn.Conv3d(24, 24, 1).eval().requires_grad_(False).to(weight_dtype)
        module.bias = torch.nn.Parameter(module.bias.float(), requires_grad=False)
        value = torch.randn(batch, 24, *geometry, dtype=input_dtype) * 0.125
        description = consumer.describe(module, value)
        assert description is not None
        # CPU refusal is before the CUDA provider/context/stream is consulted.
        assert consumer.prepare(description, context=None, stream=None) is None
        buffers = consumer.Buffers.allocate(description, 0)
        with torch.autocast("cpu", dtype=torch.float16):
            reference = module(value)
        observer = Observe()
        with observer:
            consumer.pack_input(value, buffers.input_rows)
        torch.testing.assert_close(
            buffers.input_rows,
            value.permute(0, 2, 3, 4, 1).contiguous().view(-1, 24).half(),
            rtol=0, atol=0,
        )
        # Actual CPU dense arithmetic only: this is not a fake native Plan.
        dense = torch.nn.functional.linear(
            buffers.input_rows, module.weight.view(24, 24).half(), module.bias.half()
        )
        with observer:
            consumer.unpack_output(dense, buffers.result)
        assert not any("empty" in call or "_to_copy" in call or "clone" in call
                       for call in observer.calls), observer.calls
        torch.testing.assert_close(buffers.result, reference, rtol=1e-3, atol=1e-3)
        allocated = {
            name: tensor.numel() * tensor.element_size()
            for name, tensor in vars(buffers).items() if tensor is not None
        }
        assert allocated == dict(description.allocations(0))
        rows.append({
            "shape": list(value.shape), "input_dtype": str(input_dtype),
            "weight_dtype": str(weight_dtype), "bias_dtype": str(module.bias.dtype),
            "max_abs": float((buffers.result.float() - reference.float()).abs().max()),
            "packing_ops": observer.calls, "actual_allocations": allocated,
        })

    valid = torch.nn.Conv3d(24, 24, 1).eval().requires_grad_(False)
    value = torch.randn(2, 24, 2, 3, 4)
    refused = []
    cases = [
        ("stride", torch.nn.Conv3d(24, 24, 1, stride=2).eval().requires_grad_(False), value),
        ("padding", torch.nn.Conv3d(24, 24, 1, padding=1).eval().requires_grad_(False), value),
        ("dilation", torch.nn.Conv3d(24, 24, 1, dilation=2).eval().requires_grad_(False), value),
        ("kernel", torch.nn.Conv3d(24, 24, 3).eval().requires_grad_(False), value),
        ("groups", torch.nn.Conv3d(24, 24, 1, groups=2).eval().requires_grad_(False), value),
        ("missing_bias", torch.nn.Conv3d(24, 24, 1, bias=False).eval().requires_grad_(False), value),
        ("training", torch.nn.Conv3d(24, 24, 1).requires_grad_(False), value),
        ("gradient_weights", torch.nn.Conv3d(24, 24, 1).eval(), value),
        ("channels_last", valid, value.to(memory_format=torch.channels_last_3d)),
        ("bf16", valid, value.bfloat16()),
        ("empty", valid, value[:0]),
    ]
    hooked = torch.nn.Conv3d(24, 24, 1).eval().requires_grad_(False)
    handle = hooked.register_forward_pre_hook(lambda module, args: None)
    overridden = torch.nn.Conv3d(24, 24, 1).eval().requires_grad_(False)
    overridden.forward = lambda x: x
    strided = torch.nn.Conv3d(24, 24, 1).eval().requires_grad_(False)
    strided.weight = torch.nn.Parameter(strided.weight.transpose(0, 1), requires_grad=False)
    cases.extend((("forward_hook", hooked, value), ("forward_override", overridden, value),
                  ("strided_weight", strided, value), ("gradient_input", valid, value.detach().requires_grad_())))
    for name, module, tensor in cases:
        before = tensor.clone()
        assert consumer.describe(module, tensor) is None, name
        torch.testing.assert_close(tensor, before, rtol=0, atol=0)
        refused.append(name)
    handle.remove()

assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(json.dumps({
    "passed": True, "torch": torch.__version__, "cuda_initialized": False,
    "consumer_sha256": hashlib.sha256(args.consumer.read_bytes()).hexdigest(),
    "cases": rows, "refusals": refused, "native_provider_executed": False,
    "workspace_query_executed": False, "gpu_qualified": False,
    "scope": "actual CPU Conv3d comparison and production packing/layout/declaration only",
}, indent=2))
