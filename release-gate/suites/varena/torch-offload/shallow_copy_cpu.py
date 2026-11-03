"""Real ATen compatibility and inference-mode rebind; no native resource substitutes."""

from __future__ import annotations

import copy
import json
import sys
from collections import Counter
from pathlib import Path

import torch
from diffusers import AutoencoderKL
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import _Device, _Version, _Weight

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
OP = torch.ops.aten._has_compatible_shallow_copy_type.default
QUERY = str(OP)


def observe(action):
    counts, operators = Counter(), Counter()
    watched = {
        _Version.tensor.__code__: "version_tensor",
        _Version.reservation.__code__: "reservation",
        _Weight._metadata.__code__: "metadata_tensor",
        _Device.execution.__wrapped__.__code__: "execution",
    }

    def profile(frame, event, _):
        if event != "call":
            return
        if frame.f_code is _Weight.__torch_dispatch__.__func__.__code__:
            operators[str(frame.f_locals["func"])] += 1
        if frame.f_code not in watched:
            return
        parent = frame.f_back
        while parent is not None:
            if parent.f_code is _Weight.__torch_dispatch__.__func__.__code__:
                if str(parent.f_locals["func"]) == QUERY:
                    counts[watched[frame.f_code]] += 1
                break
            parent = parent.f_back

    previous = sys.getprofile()
    sys.setprofile(profile)
    try:
        result = action()
    finally:
        sys.setprofile(previous)
    return result, dict(counts), dict(operators)


class Model(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.arange(24.0).reshape(4, 6)[1:3, 1:5].T)
        self.tied = self.weight
        self.register_buffer("bias", torch.arange(4.0))

    def forward(self, value):
        return torch.nn.functional.linear(value, self.weight, self.bias)


def outcome(left, right):
    try:
        return {"value": OP(left, right)}
    except (RuntimeError, TypeError) as error:
        return {"error_type": type(error).__name__}


model, peer = Model().eval(), Model().eval()
stock = copy.deepcopy(model)
reference, peer_reference = model.weight.detach().clone(), peer.weight.detach().clone()
identity = id(model.weight)
owner, peer_owner = vt.offload(model, device="cpu"), vt.offload(peer, device="cpu")
cases = {
    "dense": (reference, reference),
    "different_dtype": (reference.double(), reference.double()),
    "different_shape": (torch.ones(1), torch.ones(1)),
    "meta": (torch.empty(4, 2, device="meta"), torch.empty(4, 2, device="meta")),
    "sparse_coo": (reference.to_sparse(), reference.to_sparse()),
    "sparse_csr": (reference.to_sparse_csr(), reference.to_sparse_csr()),
    "quantized": (
        torch.quantize_per_tensor(reference, 0.1, 0, torch.qint8),
        torch.quantize_per_tensor(reference, 0.1, 0, torch.qint8),
    ),
    "same_managed": (model.weight, reference),
    "independent_managed": (peer.weight, peer_reference),
    "invalid_argument": (None, None),
}
rows, query_counts = [], []
for mode, context in (
    ("no_grad", torch.no_grad),
    ("inference_mode", torch.inference_mode),
):
    with context():
        for name, (other, ordinary) in cases.items():
            for reverse in (False, True):
                actual_pair = (
                    (other, model.weight) if reverse else (model.weight, other)
                )
                expected_pair = (
                    (ordinary, reference) if reverse else (reference, ordinary)
                )
                expected = outcome(*expected_pair)
                actual, counts, _ = observe(lambda pair=actual_pair: outcome(*pair))
                rows.append(
                    {
                        "mode": mode,
                        "case": name,
                        "reverse": reverse,
                        "expected": expected,
                        "actual": actual,
                    }
                )
                query_counts.append(counts)
    assert model.weight is model.tied and id(model.weight) == identity
escaped = model.weight.detach()
old_version = escaped._version_owner
with torch.inference_mode():
    _, conversion_counts, conversion_ops = observe(lambda: model.half().float())
    stock.half().float()
    torch.testing.assert_close(
        model(torch.ones(1, 2)), stock(torch.ones(1, 2)), rtol=0, atol=0
    )
assert model.weight is model.tied and id(model.weight) == identity
assert model.weight._version_owner is not old_version
torch.testing.assert_close(escaped.clone(), reference, rtol=0, atol=0)

torch.manual_seed(19)
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
control = copy.deepcopy(vae)
vae_ids = [id(parameter) for parameter in vae.parameters()]
vae_owner = vt.offload(vae, device="cpu")
with torch.inference_mode():
    _, vae_counts, vae_ops = observe(lambda: vae.half().float())
    control.half().float()
    latent = torch.zeros(1, 2, 8, 8)
    torch.testing.assert_close(
        vae.decode(latent).sample, control.decode(latent).sample, rtol=0, atol=0
    )
assert [id(parameter) for parameter in vae.parameters()] == vae_ids
owner.close()
peer_owner.close()
vae_owner.close()
torch.testing.assert_close(escaped.clone(), reference, rtol=0, atol=0)
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
report = {
    "torch": torch.__version__,
    "cuda": False,
    "compatibility_cases": rows,
    "query_materialization_calls": query_counts,
    "conversion_materialization_calls": conversion_counts,
    "conversion_dispatch_ops": conversion_ops,
    "vae_conversion_materialization_calls": vae_counts,
    "vae_conversion_dispatch_ops": vae_ops,
    "vae_parameters": len(vae_ids),
    "tied_identity_escaped_versions_and_exact_vae_decode": True,
    "gpu_reservations_packing_and_speed": "not qualified",
}
print(json.dumps(report), flush=True)
assert all(row["actual"] == row["expected"] for row in rows)
assert not any(query_counts) and not conversion_counts and not vae_counts
assert conversion_ops.get(QUERY, 0) > 0 and vae_ops.get(QUERY, 0) > 0
