"""Actual local stock SDXL repeated denoise; operator-only ordinary Cozy client.

Run two complete calls per owning process; compare eager/auto call-N/output.npy.
No weights download, private reserve, budget estimate or inference retry.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import time
from pathlib import Path

import diffusers
import numpy as np
import torch
import transformers
from diffusers import StableDiffusionXLPipeline
from tensorfs.varena import api
from tensorfs.varena import torch as vt


def phase_samples(facts, device):
    return {
        name: vt.phase_stats(device, row["native_key"])
        for name, row in facts["boundaries"].items()
        if row["native_key"] is not None
    }


def run_call(args, pipe, owner, config_hashes, call_number):
    output = args.output / f"call-{call_number}"
    output.mkdir(parents=True, exist_ok=True)
    before = owner.graph_facts()
    entries_before = owner.component_facts()["entries"]
    starts, ends, wall_starts, wall_ends = [], [], [], []
    first_step_facts = None
    first_step_samples = None

    def denoise_start(module, inputs):
        event = torch.cuda.Event(enable_timing=True)
        event.record(torch.cuda.current_stream(args.device))
        starts.append(event)
        wall_starts.append(time.perf_counter())

    def step_end(pipeline, step, timestep, callback_kwargs):
        nonlocal first_step_facts, first_step_samples
        event = torch.cuda.Event(enable_timing=True)
        event.record(torch.cuda.current_stream(args.device))
        ends.append(event)
        wall_ends.append(time.perf_counter())
        if step == 0:
            first_step_facts = owner.graph_facts()
            first_step_samples = phase_samples(first_step_facts, args.device)
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
        previous = {
            name: row
            for name, row in before["boundaries"].items()
            if name.startswith("unet.")
        }
        assert denoise.keys() == previous.keys()
        deltas = {
            name: {
                counter: row[counter] - previous[name][counter]
                for counter in ("captures", "replays", "eager_calls")
            }
            for name, row in denoise.items()
        }
        captures = sum(row["captures"] for row in deltas.values())
        replays = sum(row["replays"] for row in deltas.values())
        reused = [
            name for name, row in deltas.items() if row["replays"] > row["captures"]
        ]
        prior_definitions_reused = [
            name
            for name, row in deltas.items()
            if previous[name]["cache"] is not None
            and previous[name]["cache"]["has_definition"]
            and row["captures"] == 0
            and row["replays"] > 0
            and previous[name]["native_key"] == denoise[name]["native_key"]
        ]
        if args.graphs == "auto":
            if call_number == 1:
                assert captures > 0, facts
            assert replays > captures and reused, facts
        else:
            assert captures == replays == 0
        entry_deltas = {
            name: count - entries_before.get(name, 0)
            for name, count in entries["entries"].items()
        }
        assert entry_deltas.get("vae.decode", 0) > 0
        assert not owner._intent_frames and not owner._intent_failed
        stage_ms = [start.elapsed_time(end) for start, end in zip(starts, ends)]
        report = {
            "passed": True,
            "scope": "one complete stock pipeline call; paired output/performance comparison still required",
            "pipeline_call": call_number,
            "phase": "first" if call_number == 1 else "warm_same_owner",
            "process_id": os.getpid(),
            "offload_owner_id": id(owner),
            "native_gpu_handle": owner._owner.gpu._handle,
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
            "weight_variant": "fp16",
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
            "unet_boundary_deltas": deltas,
            "unet_prior_definitions_reused": prior_definitions_reused,
            "component_entries": entries,
            "component_entry_deltas": entry_deltas,
            "vae_encode_entries": entry_deltas.get("vae.encode", 0),
            "vae_decode_entries": entry_deltas.get("vae.decode", 0),
            "vae_encode_scope": "not exercised by stock txt2img; use real sdxl_components.py --device cuda:0 encode/decode gate",
            "first_step": first_step_facts,
            "first_step_native_samples": first_step_samples,
            "before_call": before,
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
            "pipeline_call": call_number,
            "elapsed_s": time.perf_counter() - begin,
            "first_step": first_step_facts,
            "first_step_native_samples": first_step_samples,
        }
        try:
            failure["facts"] = owner.graph_facts()
            failure["entries"] = owner.component_facts()
            failure["native_phase_samples"] = phase_samples(
                failure["facts"], args.device
            )
            failure["working_stats"] = vt.stats(args.device)
        except RuntimeError as facts_error:
            failure["facts_error"] = str(facts_error)
        (output / "failure.json").write_text(json.dumps(failure, indent=2) + "\n")
        raise  # Receipt only; never reset owners or repeat authored inference.
    finally:
        hook.remove()
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def run(args):
    args.output.mkdir(parents=True, exist_ok=True)
    vt.install(args.device)  # Before model code can initialize CUDA.
    pipe = StableDiffusionXLPipeline.from_pretrained(
        args.model,
        torch_dtype=torch.float16,
        variant="fp16",
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
    calls = []
    try:
        for call_number in (1, 2):
            calls.append(run_call(args, pipe, owner, config_hashes, call_number))
        owner.close()  # Exactly once, after BOTH pipeline calls finish.
        assert api.retirement_stats()["quarantined"] == quarantine_before
    except BaseException as error:
        failure = {
            "type": type(error).__name__,
            "message": str(error),
            "completed_calls": calls,
            "graphs": args.graphs,
        }
        (args.output / "failure.json").write_text(json.dumps(failure, indent=2) + "\n")
        raise  # No close/reset/retry after an uncertain authored call.
    report = {
        "passed": True,
        "scope": "two complete stock calls on ONE owning handle/process; paired mode comparison remains required, no speed pass from one pair",
        "graphs": args.graphs,
        "call_count": 2,
        "steps": args.steps,
        "height": args.height,
        "width": args.width,
        "seed": args.seed,
        "weight_variant": "fp16",
        "calls": calls,
        "sum_inference_s": sum(call["elapsed_inference_s"] for call in calls),
        "timing_boundary": "each call includes prompt/all configured denoise steps/VAE/postprocessing/completion; excludes load/adoption and output/facts writes; sum excludes between-call receipt work",
    }
    (args.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
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
