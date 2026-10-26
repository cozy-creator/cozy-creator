"""Tiny real Qwen checkpoint/placement/cached interval, not a full image benchmark."""

from __future__ import annotations

import copy
import gc
import hashlib
import importlib.metadata
import json
import tempfile
import time
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
from diffusers import FlowMatchEulerDiscreteScheduler, QwenImage21Transformer2DModel
from diffusers.models.transformers.transformer_qwenimage21 import QwenImage21KVCache

from ._candidate import ResidentDenoise, _Layout

CONFIG = {
    "patch_size": 1,
    "in_channels": 16,
    "out_channels": 16,
    "num_layers": 2,
    "attention_head_dim": 64,
    "num_attention_heads": 2,
    "context_in_dim": 64,
    "mlp_ratio": 2,
    "axes_dims_rope": (16, 24, 24),
}


def place(memory, checkpoint, device, rope):
    source = Checkpoint(checkpoint)
    rows = source.rows("transformer")
    facts = measure_device(torch, device.index or 0, "cuda")
    resolution = resolve(
        variant=Variant(
            "qwen-retained", "fixture", checkpoint.id, {"transformer": checkpoint.id}
        ),
        rows=rows,
        providers=launch_providers(),
        device=facts,
        runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
        construction=ConstructionFacts(
            "qwen-retained-proof", "TinyQwen", ("transformer",)
        ),
        encoded_leaves="refuse",
        dtype_name=dtype_name,
    )
    with torch.device("meta"):
        transformer = QwenImage21Transformer2DModel(**CONFIG).to(torch.bfloat16).eval()
    # Production also constructs rotary frequencies on the CPU. They are derived
    # constants, not checkpoint weights; do not leave meta-only arrays in the fixture.
    transformer.pos_embed.freqs = [x.clone() for x in rope]
    built = SimpleNamespace(transformer=transformer)
    walked = census(built)
    backend = Backend(
        {"transformer": source},
        rows,
        torch=torch,
        plan=resolution,
        memory=memory,
        device=device,
        construction="TinyQwen",
        device_facts=facts,
        release="qwen-retained-proof",
    )
    backend.expect({"transformer": [r.key for r in rows]})
    fitted = backend.fit(walked, encoded_leaves="refuse")
    assert fitted and fitted["ok"], fitted
    live = backend.materialize(built, walked, encoded_leaves="refuse")
    for destination in walked.destinations:
        backend.fill(destination.key, destination.spec, live[destination.key])
    backend.commit()
    return transformer, backend, Placement(memory, backend)


def inputs(transformer, seed, steps, device):
    generator = torch.Generator(device=device).manual_seed(seed)
    with torch.inference_mode(False), torch.no_grad():
        latent = torch.randn(
            (1, 16, 16), generator=generator, device=device, dtype=torch.bfloat16
        )
        prompt = torch.randn(
            (1, 3, 64), generator=generator, device=device, dtype=torch.bfloat16
        )
        mask = torch.cat(
            (
                torch.zeros((1, 3), device=device, dtype=torch.bool),
                torch.ones((1, 4), device=device, dtype=torch.bool),
            ),
            dim=1,
        )
        shapes = [[(1, 4, 4)]]
        layout = _Layout(transformer, shapes, mask, None, device)
        scheduler = FlowMatchEulerDiscreteScheduler()
        scheduler.set_timesteps(steps, device=device)
        scheduler.set_begin_index(0)
        cache = QwenImage21KVCache(len(transformer.transformer_blocks))
        noise = transformer(
            hidden_states=latent,
            encoder_hidden_states=prompt,
            timestep=scheduler.timesteps[:1].to(latent.dtype) / 1000,
            img_shapes=shapes,
            img_mask=mask,
            encoder_hidden_states_mask=None,
            attention_kwargs={},
            kv_cache=cache,
            kv_cache_mode="extract",
            return_dict=False,
        )[0]
        latent = scheduler.step(
            noise[:, -latent.shape[1] :],
            scheduler.timesteps[0],
            latent,
            return_dict=False,
        )[0]
    return (
        {
            "transformer": transformer,
            "scheduler": scheduler,
            "latents": latent,
            "kv_cache": cache,
            "layout": layout,
        },
        prompt,
        mask,
        shapes,
    )


def eager(parameters, prompt, mask, shapes):
    scheduler = copy.deepcopy(parameters["scheduler"])
    latent = parameters["latents"].clone()
    with torch.no_grad():
        for timestep in scheduler.timesteps[1:]:
            noise = parameters["transformer"](
                hidden_states=latent,
                encoder_hidden_states=prompt,
                timestep=timestep.reshape(1).to(latent.dtype) / 1000,
                img_shapes=shapes,
                img_mask=mask,
                encoder_hidden_states_mask=None,
                attention_kwargs={},
                kv_cache=parameters["kv_cache"],
                kv_cache_mode="cached",
                return_dict=False,
            )[0]
            latent = scheduler.step(
                noise[:, -latent.shape[1] :], timestep, latent, return_dict=False
            )[0]
    return latent


def exercise(mode, seed, ctx, tel):
    assert mode in ("pressure", "cancel")
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    device = torch.device(str(ctx.device))
    memory = residency.Memory.open(str(device))
    source_receipt = json.loads(Path(__file__).with_name("SOURCE.json").read_text())
    assert (
        hashlib.sha256(
            Path(__file__).with_name("_candidate.py").read_bytes()
        ).hexdigest()
        == source_receipt["sha256"]
    )
    torch.manual_seed(seed)
    original = QwenImage21Transformer2DModel(**CONFIG).to(torch.bfloat16).eval()
    with torch.no_grad():
        for parameter in original.parameters():
            parameter.uniform_(-0.04, 0.04)
    rope = [x.clone() for x in original.pos_embed.freqs]
    cache = ResidentDenoise()
    with tempfile.TemporaryDirectory(prefix="cozy-qwen-retained-") as directory:
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        for name, value in original.state_dict().items():
            writer.component("transformer").put(name, value)
        checkpoint = writer.commit()
        del original
        transformer, backend, placement = place(memory, checkpoint, device, rope)
        try:
            request = residency._REQUEST.get()
            assert request is not None and request.memory is memory
            request.plan([backend.models["transformer"]])
            steps = 30 if mode == "pressure" else 1001
            with placement.using("transformer", ("transformer",)), torch.no_grad():
                parameters, prompt, mask, shapes = inputs(
                    transformer, seed, steps, device
                )
                expected = (
                    eager(parameters, prompt, mask, shapes)
                    if mode == "pressure"
                    else None
                )
                first = cache.run(**parameters, ctx=ctx, tel=tel)
                assert first is not None and torch.isfinite(first).all()
                if expected is not None:
                    torch.testing.assert_close(first, expected, rtol=0, atol=0)
                first_plan = cache._program.plan
                assert first_plan.valid()
                repeated = cache.run(**parameters, ctx=ctx, tel=tel)
                assert cache._program.plan is first_plan
                torch.testing.assert_close(repeated, first, rtol=0, atol=0)
            if mode == "pressure":
                del parameters, prompt, mask, shapes
                gc.collect()
                before = memory.native.stats()
                assert before["pinned_bytes"] > 0, before
                released = memory.trim()
                after = memory.native.stats()
                assert not first_plan.valid(), (
                    "idle pressure did not retire retained Qwen definition"
                )
                assert after["pinned_bytes"] == 0, after
                with placement.using("transformer", ("transformer",)), torch.no_grad():
                    parameters, prompt, mask, shapes = inputs(
                        transformer, seed, steps, device
                    )
                    successor = cache.run(**parameters, ctx=ctx, tel=tel)
                    assert cache._program.plan is not first_plan
                    torch.testing.assert_close(successor, first, rtol=0, atol=0)
                detail = {
                    "ordinary_eager_exact": True,
                    "same_definition_replayed": True,
                    "pressure_retired_definition": True,
                    "before": before,
                    "after": after,
                    "trim_reserved_bytes_returned": released,
                    "recaptured_successor_exact": True,
                }
            else:
                # Cancel the actual retained Qwen interval after native progress, not a
                # fabricated context flag. Keep the ordinary executor request healthy.
                with torch.no_grad():
                    cache._program.update(
                        (
                            parameters["latents"],
                            *[ref() for ref in cache._program._inputs[1:]],
                        )
                    )
                execution = ctx.plan(first_plan)
                until = time.monotonic() + 30
                progress = execution.poll()
                while (
                    not progress.done
                    and progress.completed_steps < 1
                    and time.monotonic() < until
                ):
                    time.sleep(0.001)
                    progress = execution.poll()
                assert not progress.done and progress.completed_steps > 0, (
                    "interval completed before cancellation could be observed"
                )
                execution.cancel()
                execution.wait()
                progress = execution.poll()
                assert progress.canceled and progress.completed_steps < steps - 1
                cache.close()
                assert not first_plan.valid()
                with placement.using("transformer", ("transformer",)), torch.no_grad():
                    successor = cache.run(**parameters, ctx=ctx, tel=tel)
                    torch.testing.assert_close(successor, first, rtol=0, atol=0)
                detail = {
                    "same_definition_replayed": True,
                    "cancel_completed_steps": progress.completed_steps,
                    "cancel_joined": execution.completion_proven(),
                    "recaptured_successor_exact": True,
                    "scope": "Real retained interval cancellation, not wholeCLIrequest cancellation.",
                }
            cache.close()
            placement.release()
            memory.trim()
            assert memory.native.stats()["pinned_bytes"] == 0
            return {
                "mode": mode,
                "seed": seed,
                "runtime_source": _build_provenance.COMMIT,
                "device": str(device),
                "candidate": source_receipt,
                "steps": steps,
                "detail": detail,
                "scope": "Real tinyQwen checkpoint/placement/nativehelper lifecycle component, nofullimage orperformanceproof.",
            }
        finally:
            cache.close()
            placement.release()
            backend.close()
            memory.trim()
            checkpoint.close()
