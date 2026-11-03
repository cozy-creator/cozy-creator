"""Actual local stock SDXL repeated denoise; operator-only ordinary Cozy client.

Run eager/auto separately with identical settings, then compare saved output.npy.
No weights download, private reserve, budget estimate or inference retry.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

import diffusers
import numpy as np
import torch
import transformers
from diffusers import StableDiffusionXLPipeline
from tensorfs.varena import api
from tensorfs.varena import torch as vt


def run(args):
    output = args.output
    output.mkdir(parents=True, exist_ok=True)
    vt.install(args.device)  # Before model code can initialize CUDA.
    pipe = StableDiffusionXLPipeline.from_pretrained(
        args.model,
        torch_dtype=torch.float16,
        use_safetensors=True,
        local_files_only=True,
        add_watermarker=False,
    )
    for component in pipe.components.values():
        if isinstance(component, torch.nn.Module):
            component.eval()
    pipe.set_progress_bar_config(disable=True)
    owner = vt.offload(pipe, device=args.device, graphs=args.graphs)
    config_hashes = {
        name: hashlib.sha256(
            (args.model / name / "config.json").read_bytes()
        ).hexdigest()
        for name in ("unet", "vae", "text_encoder", "text_encoder_2")
    }
    quarantine_before = api.retirement_stats()["quarantined"]
    starts, ends, wall_starts, wall_ends = [], [], [], []
    first_step_facts = None

    def denoise_start(module, inputs):
        event = torch.cuda.Event(enable_timing=True)
        event.record(torch.cuda.current_stream(args.device))
        starts.append(event)
        wall_starts.append(time.perf_counter())

    def step_end(pipeline, step, timestep, callback_kwargs):
        nonlocal first_step_facts
        event = torch.cuda.Event(enable_timing=True)
        event.record(torch.cuda.current_stream(args.device))
        ends.append(event)
        wall_ends.append(time.perf_counter())
        if step == 0:
            first_step_facts = owner.graph_facts()
        return callback_kwargs  # Exact original objects/aliases.

    # Ancestor timing hook executes once per UNet call. It is outside every
    # selected child's graph definition; no child hook or body is substituted.
    hook = pipe.unet.register_forward_pre_hook(denoise_start)
    generator = torch.Generator(device="cpu").manual_seed(args.seed)
    begin = time.perf_counter()
    try:
        with torch.inference_mode():
            result = pipe(
                prompt=args.prompt,
                negative_prompt="",
                height=args.height,
                width=args.width,
                num_inference_steps=args.steps,
                guidance_scale=args.guidance,
                generator=generator,
                output_type="np",
                callback_on_step_end=step_end,
            )
        assert len(starts) == len(ends) == args.steps
        ends[-1].synchronize()  # Selected caller-stream completion, no device sync.
        elapsed = time.perf_counter() - begin
        images = result.images
        assert type(images) is np.ndarray and np.isfinite(images).all()
        np.save(output / "output.npy", images, allow_pickle=False)
        facts = owner.graph_facts()
        entries = owner.component_facts()
        denoise = {
            name: row
            for name, row in facts["boundaries"].items()
            if name.startswith("unet.")
        }
        captures = sum(row["captures"] for row in denoise.values())
        replays = sum(row["replays"] for row in denoise.values())
        reused = [
            name for name, row in denoise.items() if row["replays"] > row["captures"]
        ]
        if args.graphs == "auto":
            assert captures > 0 and replays > captures and reused, facts
        else:
            assert captures == replays == 0
        assert entries["entries"].get("vae.decode", 0) > 0
        assert not owner._intent_frames and not owner._intent_failed
        stage_ms = [start.elapsed_time(end) for start, end in zip(starts, ends)]
        report = {
            "passed": True,
            "scope": "one actual stock local SDXL mode; paired output/performance comparison still required",
            "graphs": args.graphs,
            "torch": torch.__version__,
            "diffusers": diffusers.__version__,
            "transformers": transformers.__version__,
            "model": str(args.model),
            "model_config_sha256": config_hashes,
            "prompt": args.prompt,
            "seed": args.seed,
            "height": args.height,
            "width": args.width,
            "steps": args.steps,
            "guidance": args.guidance,
            "weight_dtype": "float16",
            "inference_mode": True,
            "elapsed_inference_s": elapsed,
            "unet_plus_scheduler_ms": stage_ms,
            "host_observed_step_s": [
                end - start for start, end in zip(wall_starts, wall_ends)
            ],
            "timing_boundary": "per-step CUDA events: immediately before UNet to scheduler step-end; total includes prompt/VAE/postprocessing, excludes model loading/adoption/file writes; host intervals are not GPU-completion timings",
            "unet_captures": captures,
            "unet_replays": replays,
            "unet_reused_boundaries": reused,
            "component_entries": entries,
            "vae_encode_entries": entries["entries"].get("vae.encode", 0),
            "vae_decode_entries": entries["entries"].get("vae.decode", 0),
            "vae_encode_scope": "not exercised by stock txt2img; use real sdxl_components.py --device cuda:0 encode/decode gate",
            "first_step": first_step_facts,
            "final": facts,
            "output_shape": images.shape,
            "output_dtype": str(images.dtype),
            "output_sha256": hashlib.sha256(
                (output / "output.npy").read_bytes()
            ).hexdigest(),
            "native_stats": vt.stats(args.device),
        }
    except BaseException as error:
        failure = {
            "type": type(error).__name__,
            "message": str(error),
            "graphs": args.graphs,
            "elapsed_s": time.perf_counter() - begin,
        }
        try:
            failure["facts"] = owner.graph_facts()
            failure["entries"] = owner.component_facts()
        except RuntimeError as facts_error:
            failure["facts_error"] = str(facts_error)
        (output / "failure.json").write_text(json.dumps(failure, indent=2) + "\n")
        raise  # Receipt only; never reset owners or repeat authored inference.
    finally:
        hook.remove()
    owner.close()
    assert api.retirement_stats()["quarantined"] == quarantine_before
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--graphs", choices=("eager", "auto"), default="auto")
    parser.add_argument("--steps", type=int, default=20)
    parser.add_argument("--height", type=int, default=1024)
    parser.add_argument("--width", type=int, default=1024)
    parser.add_argument("--guidance", type=float, default=5.0)
    parser.add_argument("--seed", type=int, default=777)
    parser.add_argument("--prompt", default="A lighthouse above a calm ocean at sunset")
    options = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(run(options), indent=2))
