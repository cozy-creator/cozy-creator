"""Benchmark observer: count the real sampler callbacks before websocket throttling.

The sampler, callback, tensors and numerical arguments are forwarded unchanged. One
small receipt is written after sampling; there is no per-step file I/O or CUDA wait.
"""

import functools
import hashlib
import inspect
import json
import os
import time
from pathlib import Path

import comfy.samplers
from comfy_execution.utils import get_executing_context
from server import PromptServer

ROOT = Path("/var/tmp/bench4090-20261026/step-receipts")
EXPECTED = "2d2dcdee55ff336ad5920a9af9326694ed8e4512390edaf7bbb12590a4ae002a"
assert (
    hashlib.sha256(Path(inspect.getfile(comfy.samplers)).read_bytes()).hexdigest()
    == EXPECTED
)
_original = comfy.samplers.KSAMPLER.sample


@functools.wraps(_original)
def observed(
    self,
    model_wrap,
    sigmas,
    extra_args,
    callback,
    noise,
    latent_image=None,
    denoise_mask=None,
    disable_pbar=False,
):
    context = get_executing_context()
    prompt = (
        context.prompt_id
        if context is not None
        else PromptServer.instance.last_prompt_id
    )
    node = (
        context.node_id if context is not None else PromptServer.instance.last_node_id
    )
    assert prompt and all(c.isalnum() or c in "-_" for c in prompt)
    values, started, succeeded = [], time.monotonic(), False

    def step(index, denoised, state, total):
        values.append([int(index), int(total), time.monotonic() - started])
        if callback is not None:
            return callback(index, denoised, state, total)

    try:
        result = _original(
            self,
            model_wrap,
            sigmas,
            extra_args,
            step,
            noise,
            latent_image,
            denoise_mask,
            disable_pbar,
        )
        succeeded = True
        return result
    finally:
        ROOT.mkdir(parents=True, exist_ok=True)
        path = ROOT / f"{prompt}-{node}.json"
        temporary = path.with_suffix(".tmp")
        temporary.write_text(
            json.dumps(
                {
                    "prompt_id": prompt,
                    "node": str(node),
                    "pid": os.getpid(),
                    "seed": extra_args.get("seed"),
                    "sigma_points": len(sigmas),
                    "callbacks": values,
                    "succeeded": succeeded,
                    "samplers_sha256": EXPECTED,
                    "scope": "Actual server sampler callbacks; host enqueue timing, not CUDA kernel duration.",
                }
            )
        )
        temporary.replace(path)


comfy.samplers.KSAMPLER.sample = observed
NODE_CLASS_MAPPINGS = {}
