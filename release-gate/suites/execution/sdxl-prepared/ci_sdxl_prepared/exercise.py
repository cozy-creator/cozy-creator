"""Real UNet weights, TensorFS checkpoint, production Backend and native Euler execution."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace

import tensorfs
import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author._loader import census
from cozy_runtime.internal import accel, residency
from cozy_runtime.internal.encoding import launch_providers
from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement
from diffusers import EulerDiscreteScheduler, UNet2DConditionModel

from ._candidate import repeat_euler

CONFIG = {
    "sample_size": 8,
    "in_channels": 4,
    "out_channels": 4,
    "layers_per_block": 1,
    "block_out_channels": (32, 64),
    "down_block_types": ("CrossAttnDownBlock2D", "DownBlock2D"),
    "up_block_types": ("UpBlock2D", "CrossAttnUpBlock2D"),
    "cross_attention_dim": 32,
    "attention_head_dim": 8,
    "norm_num_groups": 8,
    "addition_embed_type": "text_time",
    "addition_time_embed_dim": 8,
    "projection_class_embeddings_input_dim": 80,
}


def place(memory, checkpoint, device):
    opened = Checkpoint(checkpoint)
    rows = opened.rows("unet")
    facts = measure_device(torch, device.index or 0, "cuda")
    plan = resolve(
        variant=Variant(
            "sdxl-prepared", "fixture", checkpoint.id, {"unet": checkpoint.id}
        ),
        rows=rows,
        providers=launch_providers(),
        device=facts,
        runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
        construction=ConstructionFacts("sdxl-prepared-proof", "SmallUNet", ("unet",)),
        encoded_leaves="refuse",
        dtype_name=dtype_name,
    )
    with torch.device("meta"):
        unet = UNet2DConditionModel(**CONFIG).half().eval()
    built = SimpleNamespace(unet=unet)
    walked = census(built)
    backend = Backend(
        {"unet": opened},
        rows,
        torch=torch,
        plan=plan,
        memory=memory,
        device=device,
        construction="SmallUNet",
        device_facts=facts,
        release="sdxl-prepared-proof",
    )
    backend.expect({"unet": [row.key for row in rows]})
    fit = backend.fit(walked, encoded_leaves="refuse")
    assert fit and fit["ok"], fit
    live = backend.materialize(built, walked, encoded_leaves="refuse")
    for destination in walked.destinations:
        backend.fill(destination.key, destination.spec, live[destination.key])
    backend.commit()
    return unet, backend, Placement(memory, backend)


def schedule(steps, device, prediction="epsilon"):
    scheduler = EulerDiscreteScheduler(
        num_train_timesteps=1000,
        beta_start=0.00085,
        beta_end=0.012,
        beta_schedule="scaled_linear",
        prediction_type=prediction,
    )
    scheduler.set_timesteps(steps, device=device)
    return scheduler


def eager(unet, scheduler, latent, inputs, guidance, count):
    prompt, pooled, time_ids = inputs
    with torch.no_grad():
        for timestep in scheduler.timesteps[:count]:
            batch = torch.cat([latent] * 2) if guidance > 1 else latent
            noise = unet(
                scheduler.scale_model_input(batch, timestep),
                timestep,
                encoder_hidden_states=prompt,
                added_cond_kwargs={"text_embeds": pooled, "time_ids": time_ids},
            ).sample
            if guidance > 1:
                uncond, cond = noise.chunk(2)
                noise = uncond + guidance * (cond - uncond)
            latent = scheduler.step(noise, timestep, latent).prev_sample
    return latent


def exercise(seed, ctx, tel):
    torch.set_num_threads(1)
    device = torch.device(str(ctx.device))
    assert device.type == "cuda"
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(str(device))
    memory.apply_budget()
    source = json.loads(Path(__file__).with_name("source.json").read_text())
    assert (
        source["source_sha256"]
        == hashlib.sha256(
            Path(__file__).with_name("_candidate.py").read_bytes()
        ).hexdigest()
    )
    report = {
        **source,
        "seed": seed,
        "runtime_commit": _build_provenance.COMMIT,
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": tensorfs.__version__,
        "torch": torch.__version__,
        "device": torch.cuda.get_device_name(device),
        "cases": [],
    }
    generator = torch.Generator().manual_seed(seed)
    with tempfile.TemporaryDirectory(prefix="cozy-sdxl-prepared-") as directory:
        with torch.random.fork_rng(devices=[]):
            torch.manual_seed(seed)
            source_unet = UNet2DConditionModel(**CONFIG).half().eval()
        writer = tensorfs.Writer(Path(directory) / "unet.cozytensors")
        component = writer.component("unet")
        for key, value in source_unet.state_dict().items():
            component.put(key, value)
        checkpoint = writer.commit()
        del source_unet
        unet, backend, placement = place(memory, checkpoint, device)
        try:
            for guidance in (1.0, 7.0):
                batch = 2 if guidance > 1 else 1
                inputs = (
                    torch.randn(batch, 4, 32, generator=generator).half().to(device),
                    torch.randn(batch, 32, generator=generator).half().to(device),
                    torch.tensor(
                        [[64, 64, 0, 0, 64, 64]] * batch,
                        dtype=torch.float16,
                        device=device,
                    ),
                )
                noise = torch.randn(1, 4, 8, 8, generator=generator).half().to(device)
                for steps in (2, 7, 30):
                    placement.open_attempt()
                    with memory.request(f"euler:{guidance}:{steps}") as request:
                        placement.plan(("unet",), request)
                        oracle_scheduler = schedule(steps, device)
                        initial = noise * oracle_scheduler.init_noise_sigma
                        with placement.using("eager", ("unet",)):
                            expected = eager(
                                unet, oracle_scheduler, initial, inputs, guidance, steps
                            )
                        native_scheduler = schedule(steps, device)
                        with placement.using("step0", ("unet",)):
                            first = eager(
                                unet, native_scheduler, initial, inputs, guidance, 1
                            )
                        before = first.clone()
                        actual = repeat_euler(
                            unet=unet,
                            scheduler=native_scheduler,
                            latents=first,
                            prompt=inputs[0],
                            pooled=inputs[1],
                            time_ids=inputs[2],
                            guidance=guidance,
                            ctx=ctx,
                            tel=tel,
                        )
                        assert actual is not None, (
                            "resident native interval was refused"
                        )
                        assert torch.equal(first, before), (
                            "capture changed caller's eager latent"
                        )
                        error = float((actual.float() - expected.float()).abs().max())
                        assert torch.equal(actual, expected), {
                            "steps": steps,
                            "guidance": guidance,
                            "max_abs_error": error,
                        }
                        report["cases"].append(
                            {
                                "steps": steps,
                                "guidance": guidance,
                                "max_abs_error": error,
                            }
                        )
                    placement.release()
                    assert memory.native.stats()["pinned_bytes"] == 0

            # Capacity admission must never read evicted weights just to capture a graph.
            memory.trim()
            assert memory.native.stats()["resident_bytes"] == 0
            scheduler = schedule(30, device)
            scheduler.set_begin_index(1)
            scheduler.scale_model_input(noise, scheduler.timesteps[1])
            with memory.request("nonresident") as request:
                placement.plan(("unet",), request)
                refused = repeat_euler(
                    unet=unet,
                    scheduler=scheduler,
                    latents=noise,
                    prompt=inputs[0],
                    pooled=inputs[1],
                    time_ids=inputs[2],
                    guidance=7.0,
                    ctx=ctx,
                    tel=tel,
                )
                assert refused is None
            assert request.stats()["moved"]["store"]["bytes"] == 0
            report["nonresident_zero_read_refusal"] = True
            unsupported = schedule(30, device, prediction="v_prediction")
            unsupported.set_begin_index(1)
            unsupported.scale_model_input(noise, unsupported.timesteps[1])
            assert (
                repeat_euler(
                    unet=unet,
                    scheduler=unsupported,
                    latents=noise,
                    prompt=inputs[0],
                    pooled=inputs[1],
                    time_ids=inputs[2],
                    guidance=7.0,
                    ctx=ctx,
                    tel=tel,
                )
                is None
            )
            report["unsupported_prediction_refused"] = True
        finally:
            placement.release()
            backend.close()
            memory.trim()
            checkpoint.close()
    return report
