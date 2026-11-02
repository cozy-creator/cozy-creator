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


# Task-local fixed-two-pass observation successor. Pipeline math is unchanged.
observations = {}


def snapshot(label, owner=None):
    observation = {"stats": vt.stats(args.device)}
    if owner is not None:
        observation["intent_facts"] = owner.intent_facts()
    observations[label] = observation
    (args.output / "observations.json").write_text(
        json.dumps(observations, indent=2) + "\n"
    )


def flatten_numbers(value, prefix=""):
    if isinstance(value, dict):
        result = {}
        for key, item in value.items():
            result.update(flatten_numbers(item, f"{prefix}.{key}" if prefix else key))
        return result
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return {prefix: value}
    return {}


started = time.perf_counter()
pipe.to(device)
timings["baseline_to_cuda_call_s"] = time.perf_counter() - started
baseline = []
for index in range(2):
    label = f"baseline-{index + 1}"
    snapshot(label + "-before")
    started = time.perf_counter()
    images = run()
    timings[label + "_generation_to_numpy_s"] = time.perf_counter() - started
    snapshot(label + "-after")
    preserve(label, images)
    assert pipe.vae.config.force_upcast
    assert pipe.vae.dtype == torch.float16
    baseline.append(images)
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
adopted = []
for index in range(2):
    label = f"adopted-{index + 1}"
    snapshot(label + "-before")
    started = time.perf_counter()
    images = run()
    timings[label + "_generation_to_numpy_s"] = time.perf_counter() - started
    snapshot(label + "-after", owner)
    torch.cuda.synchronize(device)
    preserve(label, images)
    assert pipe.vae.config.force_upcast
    assert pipe.vae.dtype == torch.float16
    assert ids == {
        name: [id(p) for p in component.parameters()]
        for name, component in pipe.components.items()
        if isinstance(component, torch.nn.Module)
    }
    adopted.append(images)

comparisons = []
for index, (actual, expected) in enumerate(zip(adopted, baseline)):
    verdict = {
        "pair": index + 1,
        "baseline": f"baseline-{index + 1}",
        "adopted": f"adopted-{index + 1}",
        "rtol": 1e-3,
        "atol": 1e-3,
        "shape": list(actual.shape),
        "max_abs_error": float(np.max(np.abs(actual - expected))),
    }
    try:
        np.testing.assert_allclose(actual, expected, rtol=1e-3, atol=1e-3)
    except AssertionError as error:
        verdict.update(passed=False, original_assertion=str(error))
    else:
        verdict["passed"] = True
    comparisons.append(verdict)
deltas = {}
for label in ("baseline-1", "baseline-2", "adopted-1", "adopted-2"):
    before = flatten_numbers(observations[label + "-before"]["stats"])
    after = flatten_numbers(observations[label + "-after"]["stats"])
    deltas[label] = {key: after[key] - before[key] for key in before.keys() & after.keys()}
(args.output / "stats-deltas.json").write_text(json.dumps(deltas, indent=2) + "\n")
comparison = {
    "diagnostic": "fixed-two-baseline-then-two-adopted",
    "steps": args.steps,
    "variant": args.variant,
    "seed": 1701,
    "timings": timings,
    "comparisons": comparisons,
    "original_numerical_assertions_passed": all(v["passed"] for v in comparisons),
    "stock_sdxl_vae_cycle": "pass",
    "performance_qualification": False,
    "scope": "Two sequential baseline calls then one adoption and two adopted calls; same generation-to-NumPy timing boundary; read-only stats outside timing; no performance qualification.",
    "output_files": {
        p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(args.output.iterdir()) if p.is_file()
    },
}
(args.output / "comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")
print(json.dumps(comparison), flush=True)
assert comparison["original_numerical_assertions_passed"], comparisons
