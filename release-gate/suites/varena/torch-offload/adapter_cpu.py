"""Real public offload frontend integration; CPU Source execution, no CUDA."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from diffusers import AutoencoderKL, DiffusionPipeline
from tensorfs.varena import torch as vt
from tensorfs.varena._offload import OffloadError

torch.set_num_threads(1)
torch.set_num_interop_threads(1)
torch.manual_seed(1701)
checks = []


class Tied(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.left = torch.nn.Linear(3, 2, bias=False)
        self.right = torch.nn.Linear(3, 2, bias=False)
        self.right.weight = self.left.weight
        self.register_buffer("state", torch.tensor(1.0))

    def forward(self, x):
        return self.left(x) + self.right(x) + self.state


model = Tied().eval()
x = torch.arange(3.0).reshape(1, 3)
with torch.no_grad():
    expected = model(x)
    expected_direct = torch.nn.functional.linear(x, model.left.weight)
parameter = model.left.weight
parameter.user_metadata = "preserved"
identity = id(parameter)
owner = vt.offload(model, device="cpu")
assert id(model.right.weight) == id(parameter) == identity
assert parameter.user_metadata == "preserved"
assert torch.equal(owner(x), expected)
assert torch.equal(torch.nn.functional.linear(x, parameter), expected_direct)
checks.append(
    "public CPU adoption preserves tied identity, metadata, exact forward/direct reads"
)
old = parameter.t().detach()[:, :1]
old_values = old.clone()
assert old.dtype == torch.float32
model.half()
assert parameter.dtype == torch.float16 and old.dtype == torch.float32
assert torch.equal(old.clone(), old_values)
assert model.right.weight is parameter
model.float()
assert parameter.dtype == torch.float32 and parameter.user_metadata == "preserved"
assert torch.equal(old.clone(), old_values)
checks.append(
    "controlled half/float preserves ties, identity and escaped old-dtype aliases"
)


def refused(fn):
    try:
        fn()
    except (OffloadError, RuntimeError, ValueError, TypeError):
        return
    raise AssertionError("unsupported operation accepted")


before = parameter.clone()
for fn in (
    lambda: parameter.add_(1),
    lambda: old.copy_(torch.zeros_like(old_values)),
    lambda: setattr(parameter, "data", torch.zeros_like(before)),
    lambda: parameter.requires_grad_(True),
    lambda: setattr(parameter, "requires_grad", True),
    lambda: torch.add(parameter, 1, out=parameter),
    lambda: parameter.data_ptr(),
    lambda: torch.Tensor.data_ptr(parameter),
    lambda: parameter.untyped_storage(),
    lambda: parameter.__dlpack__(),
    lambda: model.train(),
    lambda: model(torch.ones(1, 3, requires_grad=True)),
    lambda: model._apply(lambda t: t + 1),
    lambda: model.to("meta"),
    lambda: torch.ops.aten.native_batch_norm.default(
        torch.ones(2, 3), None, None, parameter[0], parameter[1], True, 0.1, 1e-5
    ),
):
    refused(fn)
assert torch.equal(before, parameter.clone())
copy = parameter.clone()
copy.add_(2)
assert type(copy) is torch.Tensor and torch.equal(parameter.clone(), before)
unsafe = torch.ops.aten._unsafe_view.default(parameter, [6])
refused(lambda: unsafe.add_(1))
assert torch.equal(unsafe.clone(), before.reshape(6))
checks.append(
    "managed mutation/training/raw-storage refused; independent copies remain mutable"
)

# Actual distinct Parameters sharing a transposed/interior storage.
base = torch.arange(20.0).reshape(4, 5)
views = torch.nn.Module().eval()
views.a = torch.nn.Parameter(base[1:, 1:4].T, requires_grad=False)
views.b = torch.nn.Parameter(base[2:, 2:], requires_grad=False)
views.empty = torch.nn.Parameter(torch.empty(0), requires_grad=False)
a, b = views.a.clone(), views.b.clone()
view_owner = vt.offload(views, device="cpu")
base.add_(100)  # The original host storage is no longer a model-content owner.
assert torch.equal(views.a.clone(), a) and torch.equal(views.b.clone(), b)
views.double()
assert torch.equal(views.a.clone(), a.double()) and torch.equal(
    views.b.clone(), b.double()
)
assert views.empty.numel() == 0
refused(lambda: views.to(dtype=torch.int32))
assert torch.equal(views.a.clone(), a.double())
checks.append(
    "shared noncontiguous/interior storage descriptors and empty weights survive conversion"
)

transformer = torch.nn.TransformerEncoderLayer(
    8, 2, dim_feedforward=16, dropout=0, batch_first=True
).eval()
tokens = torch.randn(2, 3, 8)
with torch.no_grad():
    transformer_expected = transformer(tokens)
transformer_owner = vt.offload(transformer, device="cpu")
with torch.no_grad():
    torch.testing.assert_close(transformer(tokens), transformer_expected)
checks.append("real TransformerEncoder attention/MLP inference agrees with stock CPU")

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
    latents = vae.encode(image).latent_dist.mode()
    decoded = vae.decode(latents).sample
ids = [id(p) for p in vae.parameters()]


class Pipeline(DiffusionPipeline):
    def __init__(self, vae):
        super().__init__()
        self.register_modules(vae=vae)


pipeline = Pipeline(vae)
vae_owner = vt.offload(pipeline, device="cpu")
with torch.no_grad():
    new_latents = vae.encode(image).latent_dist.mode()
    new_decoded = vae.decode(new_latents).sample
assert torch.equal(new_latents, latents) and torch.equal(new_decoded, decoded)
# SDXL's normal VAE path converts both the root and selected children outside forward.
vae.half()
vae.to(dtype=torch.float32)
vae.post_quant_conv.to(dtype=torch.float16)
vae.decoder.conv_in.to(dtype=torch.float16)
vae.decoder.mid_block.to(dtype=torch.float16)
vae.to(dtype=torch.float16)
assert all(p.dtype == torch.float16 for p in vae.parameters())
assert [id(p) for p in vae.parameters()] == ids
assert vae.config.force_upcast is True
checks.append(
    "real Diffusers pipeline VAE encode/decode exact; root/child SDXL-shaped dtype conversions retain identity"
)

owner.close()
refused(lambda: model(x))
assert torch.equal(old.clone(), old_values)
model = owner = None
assert torch.equal(old.clone(), old_values)
checks.append(
    "closed/dropped model handle refuses entry; escaped aliases retain immutable versions"
)
assert not torch.cuda.is_initialized()
assert not any(
    "libcuda.so" in line for line in Path("/proc/self/maps").read_text().splitlines()
)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "checks": checks,
            "cuda": False,
            "native_residency": False,
        }
    )
)
