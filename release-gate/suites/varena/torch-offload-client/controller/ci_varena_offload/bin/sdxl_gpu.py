"""Actual stock SDXL comparison using an existing local model on the coordinator GPU."""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

import numpy as np
import torch
from diffusers import StableDiffusionXLPipeline
from tensorfs.varena import torch as vt

parser = argparse.ArgumentParser()
parser.add_argument("model", help="existing local SDXL directory; no downloads")
parser.add_argument("--device", type=int, default=0)
parser.add_argument(
    "--variant", default=None, help="explicit existing model variant, e.g. fp16"
)
parser.add_argument(
    "--output",
    type=Path,
    required=True,
    help="new output directory for full arrays, previews and timings",
)
parser.add_argument("--steps", type=int, default=2, help="2 is correctness smoke only")
parser.add_argument("--prompt", default="a red ceramic teapot on a wooden table")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
timings = {}


def checkpoint():
    (args.output / "timings.json").write_text(json.dumps(timings, indent=2) + "\n")


def preserve(name, images):
    from PIL import Image

    np.save(args.output / (name + ".npy"), images, allow_pickle=False)
    for index, image in enumerate(images):
        pixels = (np.clip(image, 0, 1) * 255).round().astype(np.uint8)
        Image.fromarray(pixels).save(
            args.output / f"{name}-{index}.webp", lossless=True
        )
    checkpoint()


vt.install(args.device)
device = torch.device("cuda", args.device)
started = time.perf_counter()
pipe = StableDiffusionXLPipeline.from_pretrained(
    args.model,
    torch_dtype=torch.float16,
    local_files_only=True,
    variant=args.variant,
    use_safetensors=True,
)
timings["load_local_model_s"] = time.perf_counter() - started
checkpoint()
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
started = time.perf_counter()
pipe.to(device)
timings["baseline_to_cuda_call_s"] = time.perf_counter() - started
started = time.perf_counter()
expected = run()
timings["baseline_generation_to_numpy_s"] = time.perf_counter() - started
preserve("baseline", expected)
started = time.perf_counter()
pipe.to("cpu")
torch.cuda.synchronize(device)
timings["return_to_cpu_and_completion_s"] = time.perf_counter() - started
checkpoint()
ids = {
    name: [id(p) for p in component.parameters()]
    for name, component in pipe.components.items()
    if isinstance(component, torch.nn.Module)
}
started = time.perf_counter()
owner = vt.offload(pipe, device=args.device)
timings["offload_setup_call_s"] = time.perf_counter() - started
checkpoint()
started = time.perf_counter()
actual = run()
timings["adopted_generation_to_numpy_s"] = time.perf_counter() - started
torch.cuda.synchronize(device)
preserve("adopted", actual)
comparison = {
    "steps": args.steps,
    "variant": args.variant,
    "seed": 1701,
    "shape": list(actual.shape),
    "baseline_shape": list(expected.shape),
    "max_abs_error": float(np.max(np.abs(actual - expected))),
    "timings": timings,
    "scope": "single sequential stock baseline then offload; same generation-to-NumPy timing boundary;2 steps smoke only; not performance qualification",
}
(args.output / "comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
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
            "variant": args.variant,
            "timings": timings,
            "output_files": {
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in sorted(args.output.iterdir())
                if p.is_file()
            },
            "stats": vt.stats(args.device),
        }
    )
)
