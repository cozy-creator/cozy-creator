"""Coordinator-owned actual CUDA fixture. Requires a matching installed Torch shim.

No GPU execution was performed by the adapter lane. This client downloads no models.
"""

from __future__ import annotations

import argparse
import json

import torch
from diffusers import AutoencoderKL
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("--device", type=int, default=0)
args = parser.parse_args()
torch.manual_seed(1701)


class Tied(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.first = torch.nn.Linear(8, 8)
        self.second = torch.nn.Linear(8, 8)
        self.second.weight = self.first.weight

    def forward(self, value):
        return self.first(value) + self.second(value)


model = Tied().eval()
parameter = model.first.weight
identity = id(parameter)
x = torch.randn(2, 8)
with torch.no_grad():
    expected = model(x)
    expected_direct = torch.nn.functional.linear(x, model.first.weight)
owner = vt.offload(model, device=args.device)
assert id(model.second.weight) == identity
device = torch.device("cuda", args.device)
with torch.no_grad():
    default_result = model(x.to(device))
torch.testing.assert_close(default_result.cpu(), expected)
stream = torch.cuda.Stream(device=args.device)
consumer = torch.cuda.Stream(device=args.device)
with torch.no_grad(), torch.cuda.stream(stream):
    inputs = x.to(device)
    result = model(inputs)
    old = parameter.T.detach()
    old_values = old.clone()
# A later stream needs both content ordering and lifetime recording.
consumer.wait_stream(stream)
inputs.record_stream(consumer)
result.record_stream(consumer)
old_values.record_stream(consumer)
with torch.no_grad(), torch.cuda.stream(consumer):
    direct = inputs @ old
    model.half()
    assert old.dtype == torch.float32 and parameter.dtype == torch.float16
    assert torch.equal(old.clone(), old_values)
    model.float()
    assert id(model.first.weight) == id(model.second.weight) == identity
consumer.synchronize()
torch.testing.assert_close(result.cpu(), expected)
torch.testing.assert_close(direct.cpu(), expected_direct)
owner.close()
with torch.no_grad(), torch.cuda.stream(consumer):
    after_close = old.clone()
consumer.synchronize()
assert torch.equal(after_close.cpu(), old_values.cpu())

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
    expected_vae = vae.decode(latents).sample
ids = [id(p) for p in vae.parameters()]
vae_owner = vt.offload(vae, device=args.device)
with torch.no_grad(), torch.cuda.stream(stream):
    image_gpu = image.to(device)
    actual = vae.decode(vae.encode(image_gpu).latent_dist.mode()).sample
    old_weight = next(vae.parameters()).detach()
    old_copy = old_weight.clone()
    vae.half()
    vae.to(dtype=torch.float32)
    vae.post_quant_conv.to(dtype=torch.float16)
    vae.decoder.conv_in.to(dtype=torch.float16)
    vae.decoder.mid_block.to(dtype=torch.float16)
    vae.to(dtype=torch.float16)
    assert all(p.dtype == torch.float16 for p in vae.parameters())
    assert torch.equal(old_weight.clone(), old_copy)
stream.synchronize()
torch.testing.assert_close(actual.cpu(), expected_vae, rtol=3e-4, atol=3e-4)
assert [id(p) for p in vae.parameters()] == ids and vae.config.force_upcast
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "device": torch.cuda.get_device_name(args.device),
            "tied_direct_alias_streams": "pass",
            "tiny_vae": "pass",
            "full_sdxl": "not exercised",
            "stats": vt.stats(args.device),
        }
    )
)
