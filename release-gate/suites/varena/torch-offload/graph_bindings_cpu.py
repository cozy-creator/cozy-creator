"""Actual Torch capture-binding dispatch, no graph/CUDA or allocator simulation."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import torch
from diffusers import AutoencoderKL
from tensorfs.varena import _offload as adapter
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import (
    GraphSegment,
    _CaptureBindings,
    _live_storages,
    _storage_tokens,
    _tree_signature,
)

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
torch.manual_seed(17)


class Model(torch.nn.Module):
    def __init__(self):
        super().__init__()
        storage = torch.arange(16.0).reshape(4, 4)
        self.weight = torch.nn.Parameter(storage[:, 1:3].T)
        self.tied = self.weight
        self.bias = torch.nn.Parameter(torch.arange(2.0))
        self.register_buffer("constant", torch.tensor(2.0))

    def forward(self, value):
        return torch.nn.functional.linear(value, self.weight, self.bias) + self.constant


model = Model().eval()
inputs = torch.arange(8.0).reshape(2, 4)
with torch.no_grad():
    expected = model(inputs)
owner = vt.offload(model, device="cpu")
versions = tuple(dict.fromkeys(p._version_owner for p in model.parameters()))
prepared = {
    v: v.tensor(
        (v.source.nbytes // getattr(torch, v.source.dtype).itemsize,), (1,), 0, None
    )
    for v in versions
}
binding = _CaptureBindings(owner, model, prepared, None, (inputs, {}))
materializations = []


def observe(frame, event, arg):
    if event == "call" and frame.f_code is adapter._Version.tensor.__code__:
        materializations.append(frame.f_code.co_name)


with torch.no_grad():
    sys.setprofile(observe)
    try:
        with binding.scope():
            actual = model(inputs)
            output = torch.empty((2, 4))
            assert torch.add(model.weight, 1, out=output) is output
            alias = model.weight.T.detach()
            independent = model.weight.clone()
    finally:
        sys.setprofile(None)
assert not materializations, materializations
torch.testing.assert_close(actual, expected, rtol=0, atol=0)
assert model.weight is model.tied
assert alias._version_owner is model.weight._version_owner
assert type(independent) is torch.Tensor
independent.add_(1)
with torch.no_grad():
    sys.setprofile(observe)
    try:
        model(inputs)
    finally:
        sys.setprofile(None)
assert materializations, "materialization trace positive control did not run"
checks = [
    "same capture-binding dispatch gives exact tied/strided CPU inference without new materializations",
    "managed views/detach remain immutable Versions; independent outputs remain ordinary",
]


def refused(fn, message):
    try:
        fn()
    except adapter.OffloadError as error:
        assert message in str(error), str(error)
    else:
        raise AssertionError("unsupported graph operation was accepted")


with binding.scope():
    refused(lambda: model.double(), "frozen capture binding")
    refused(lambda: model.weight.add_(1), "immutable")
    refused(lambda: model.constant.add_(1), "module buffers or inputs")
    refused(lambda: inputs.add_(1), "module buffers or inputs")
    ordinary = torch.empty(0)
    refused(lambda: ordinary.set_(model.weight), "reassign ordinary storage")
    assert not ordinary.numel()
assert model.weight.dtype == torch.float32 and model.constant.item() == 2
checks.append(
    "conversion, weight/buffer/input mutation refused before changing real contents"
)

storage_tensor = torch.arange(3.0)
detached = storage_tensor.detach()
tokens = _storage_tokens([storage_tensor])
del storage_tensor
kept = _live_storages(tokens)
assert len(kept) == 1 and kept[0]._cdata == detached.untyped_storage()._cdata
del detached
assert _live_storages(tokens), "exact surviving export Storage was not retained"
del kept
assert not _live_storages(tokens)
checks.append(
    "C10 Storage ownership covers detached aliases and preserves exact live storage without creating an unpinned tensor"
)

before = GraphSegment._parameters(model)
old = model.weight.detach()
old_values = old.clone()
model.double()
assert GraphSegment._parameters(model) != before
assert torch.equal(old.clone(), old_values)
checks.append(
    "version signature changes on real dtype conversion while old managed alias retains values"
)
refused(
    lambda: owner.capture(model, (inputs,), stream=None, reserve_bytes=0),
    "CUDA offload owner",
)
refused(lambda: _tree_signature((inputs, inputs)), "aliased graph input")
refused(
    lambda: _tree_signature(torch.ones(2, requires_grad=True)),
    "ordinary nonoverlapping",
)
refused(lambda: _tree_signature(object()), "immutable scalars")
assert _tree_signature((inputs, 2)) != _tree_signature((inputs, 3))
checks.append(
    "public CPU capture and unsupported input signatures refuse before CUDA initialization"
)
owner.close()

vae = AutoencoderKL(
    in_channels=3,
    out_channels=3,
    down_block_types=("DownEncoderBlock2D",),
    up_block_types=("UpDecoderBlock2D",),
    block_out_channels=(8,),
    layers_per_block=1,
    latent_channels=2,
    norm_num_groups=4,
    sample_size=8,
).eval()
image = torch.randn(1, 3, 8, 8)
with torch.no_grad():
    expected = vae(image).sample
vae_owner = vt.offload(vae, device="cpu")
vae_versions = tuple(dict.fromkeys(p._version_owner for p in vae.parameters()))
vae_prepared = {
    v: v.tensor(
        (v.source.nbytes // getattr(torch, v.source.dtype).itemsize,), (1,), 0, None
    )
    for v in vae_versions
}
vae_binding = _CaptureBindings(vae_owner, vae, vae_prepared, None, (image, {}))
materializations.clear()
with torch.no_grad():
    sys.setprofile(observe)
    try:
        with vae_binding.scope():
            actual = vae(image).sample
    finally:
        sys.setprofile(None)
assert not materializations
torch.testing.assert_close(actual, expected, rtol=0, atol=0)
vae_owner.close()
checks.append(
    "real tiny Diffusers VAE capture-binding body matches exact eager output with no residency materialization"
)


class Empty(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.empty = torch.nn.Parameter(torch.arange(8.0)[3:3], requires_grad=False)

    def forward(self, value):
        return value + self.empty.sum()


empty = Empty().eval()
empty_owner = vt.offload(empty, device="cpu")
empty_version = empty.empty._version_owner
assert empty_version.source.nbytes > 0
with (
    torch.no_grad(),
    _CaptureBindings(
        empty_owner, empty, {empty_version: torch.empty(0)}, None, (inputs, {})
    ).scope(),
):
    assert torch.equal(empty(inputs), inputs)
assert empty_version.region is None
empty_owner.close()
checks.append(
    "zero-element interior weights use ordinary empty binding without materializing their nonempty backing storage"
)
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "graphs_executed": False,
            "native_residency": False,
            "checks": checks,
        }
    )
)
