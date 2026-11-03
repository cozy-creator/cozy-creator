"""Actual stock SDXL comparison using an existing local model on the coordinator GPU."""

from __future__ import annotations

import argparse
import json

import numpy as np
import torch
from diffusers import StableDiffusionXLPipeline
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("model", help="existing local SDXL directory; no downloads")
parser.add_argument("--device", type=int, default=0)
parser.add_argument("--steps", type=int, default=2, help="2 is correctness smoke only")
parser.add_argument("--prompt", default="a red ceramic teapot on a wooden table")
args = parser.parse_args()
vt.install(args.device)
device = torch.device("cuda", args.device)
pipe = StableDiffusionXLPipeline.from_pretrained(
    args.model, torch_dtype=torch.float16, local_files_only=True
)
assert pipe.vae.config.force_upcast, "use stock SDXL VAE force_upcast"


def run():
    with torch.inference_mode():
        return pipe(
            prompt=args.prompt,
            num_inference_steps=args.steps,
            generator=torch.Generator(device=device).manual_seed(1701),
            output_type="np",
        ).images


# Baseline uses the same working allocator and stock pipeline, including VAE upcast.
pipe.to(device)
expected = run()
pipe.to("cpu")
torch.cuda.synchronize(device)
ids = {
    name: [id(p) for p in component.parameters()]
    for name, component in pipe.components.items()
    if isinstance(component, torch.nn.Module)
}
owner = vt.offload(pipe, device=args.device)
actual = run()
torch.cuda.synchronize(device)
assert pipe.vae.config.force_upcast
assert pipe.vae.dtype == torch.float16
assert ids == {
    name: [id(p) for p in component.parameters()]
    for name, component in pipe.components.items()
    if isinstance(component, torch.nn.Module)
}
np.testing.assert_allclose(actual, expected, rtol=1e-3, atol=1e-3)
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "device": torch.cuda.get_device_name(args.device),
            "steps": args.steps,
            "shape": list(actual.shape),
            "max_abs_error": float(np.max(np.abs(actual - expected))),
            "stock_sdxl_vae_cycle": "pass",
            "performance_qualification": False,
            "stats": vt.stats(args.device),
        }
    )
)
