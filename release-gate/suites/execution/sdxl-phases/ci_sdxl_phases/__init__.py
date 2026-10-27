"""Actual four-component SDXL sequence, using the existing stable pipeline cache."""

from __future__ import annotations

import gc
import json
import os
import shutil
import tempfile
import time
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "phases"
    steps: int = 30
    guidance: float = 7.0
    seed: int = 2630011


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import tensorfs
    import torch
    from cozy_runtime import rng
    from cozy_runtime.internal import accel, residency
    from diffusers import AutoencoderKL, EulerDiscreteScheduler, UNet2DConditionModel
    from transformers import CLIPTextConfig, CLIPTextModel, CLIPTextModelWithProjection

    from ._prepared import ResidentPipeline, _SequentialPipeline, decode_latents
    from .common import current_request, place, versions

    assert payload.mode in {"phases", "cancel", "source-error", "prepare-refusal"}
    assert payload.steps in {2, 30} and payload.guidance in {1.0, 7.0}
    run_steps = 1000 if payload.mode == "cancel" else payload.steps
    tel.log("SDXL_PHASE_GATE " + json.dumps({"pid": os.getpid(), "seed": payload.seed}))
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(str(ctx.device))
    request = current_request(memory)
    initial = memory.native.stats()
    old_window = memory.window_bytes
    torch.set_num_threads(1)
    clip_config = CLIPTextConfig(
        vocab_size=32,
        hidden_size=16,
        intermediate_size=32,
        num_hidden_layers=2,
        num_attention_heads=4,
        max_position_embeddings=8,
        projection_dim=32,
        bos_token_id=0,
        eos_token_id=2,
        pad_token_id=1,
        _attn_implementation="sdpa",
    )
    unet_config = {
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
    vae_config = {
        "in_channels": 3,
        "out_channels": 3,
        "latent_channels": 4,
        "layers_per_block": 1,
        "block_out_channels": (32, 32, 32, 32),
        "down_block_types": ("DownEncoderBlock2D",) * 4,
        "up_block_types": ("UpDecoderBlock2D",) * 4,
        "norm_num_groups": 8,
        "sample_size": 64,
        "scaling_factor": 0.18215,
        "force_upcast": True,
    }

    if payload.mode == "prepare-refusal":
        # This real VAE has only three upsampling levels. It cannot write the
        # declared SDXL64px image edge, so its final warmup must refuse after
        # the text/UNet recordings exist. No mocked capture or injected fault.
        vae_config.update(
            block_out_channels=(32, 32, 32),
            down_block_types=("DownEncoderBlock2D",) * 3,
            up_block_types=("UpDecoderBlock2D",) * 3,
        )

    class UNet(UNet2DConditionModel):
        def begin_denoise_request(self, steps, hidiffusion, seed):
            assert not hidiffusion and steps > 0

    factories = {
        "text_encoder": lambda: CLIPTextModel(clip_config).half().eval(),
        "text_encoder_2": lambda: (
            CLIPTextModelWithProjection(clip_config).half().eval()
        ),
        "unet": lambda: UNet(**unet_config).half().eval(),
        "vae": lambda: AutoencoderKL(**vae_config).half().eval(),
    }
    placements, backends, components, checkpoints = [], [], {}, []
    cache = ResidentPipeline()
    private_chunks = []
    try:
        with tempfile.TemporaryDirectory(prefix="cozy-sdxl-phases-") as folder:
            for number, (name, factory) in enumerate(factories.items()):
                with torch.random.fork_rng(devices=[]):
                    torch.manual_seed(payload.seed + number)
                    source = factory()
                writer = tensorfs.Writer(Path(folder) / f"{name}.cozytensors")
                for key, value in source.state_dict().items():
                    writer.component(name).put(key, value)
                checkpoint = writer.commit()
                if payload.mode == "source-error":
                    exported = Path(
                        tensorfs.materialize(
                            checkpoint, local_dir=Path(folder) / f"{name}-export"
                        )
                    )
                    private = Path(folder) / f"{name}-private"
                    shutil.copytree(
                        exported, private, copy_function=shutil.copyfile, symlinks=False
                    )
                    opened = tensorfs.open(str(private), local_files_only=True)
                    checkpoint.close()
                    checkpoint = opened
                    private_chunks.extend(
                        private / "chunks" / digest.removeprefix("sha256:")
                        for digest, _ in checkpoint.chunks
                    )
                checkpoints.append(checkpoint)
                del source, value
                root, backend, placement = place(memory, checkpoint, name, factory)
                components[name] = root
                backends.append(backend)
                placements.append(placement)
                placement.open_attempt()
                placement.plan((name,), request)

            models = [
                backend.models[name]
                for backend, name in zip(backends, factories, strict=True)
            ]
            request.plan(({models[0], models[1]}, models[2], models[3]))
            scheduler = EulerDiscreteScheduler(
                num_train_timesteps=1000,
                beta_start=0.00085,
                beta_end=0.012,
                beta_schedule="scaled_linear",
                prediction_type="epsilon",
            )
            tokens_a = (torch.tensor([[0, 5, 8, 2]]), torch.tensor([[0, 6, 9, 2]]))
            tokens_b = (torch.tensor([[0, 7, 10, 2]]), torch.tensor([[0, 3, 11, 2]]))
            negative = (
                (torch.tensor([[0, 4, 1, 2]]), torch.tensor([[0, 4, 1, 2]]))
                if payload.guidance > 1
                else None
            )
            wide = (
                torch.bfloat16
                if torch.cuda.is_bf16_supported(including_emulation=False)
                else torch.float32
            )

            # Real independent CLIP instances keep Transformers' hidden-state collection
            # hooks out of the captured model. Same seeded checkpoint values, no mocks.
            with torch.random.fork_rng(devices=[]):
                torch.manual_seed(payload.seed)
                oracle_first = factories["text_encoder"]().to(memory.device)
                torch.manual_seed(payload.seed + 1)
                oracle_second = factories["text_encoder_2"]().to(memory.device)

            def reference(tokens, seed):
                def encode(first, second):
                    left = oracle_first(
                        first.to(memory.device), output_hidden_states=True
                    )
                    right = oracle_second(
                        second.to(memory.device), output_hidden_states=True
                    )
                    return torch.cat(
                        [left.hidden_states[-2], right.hidden_states[-2]], dim=-1
                    ), right.text_embeds

                with torch.no_grad():
                    prompt, pooled = encode(*tokens)
                    if negative is not None:
                        neg, neg_pool = encode(*negative)
                        prompt, pooled = (
                            torch.cat([neg, prompt]),
                            torch.cat([neg_pool, pooled]),
                        )
                    schedule = EulerDiscreteScheduler.from_config(scheduler.config)
                    schedule.set_timesteps(payload.steps, device=memory.device)
                    latent = (
                        rng.noise(
                            (1, 4, 8, 8),
                            seed,
                            dtype=torch.float16,
                            device=memory.device,
                        )
                        * schedule.init_noise_sigma
                    )
                    ids = torch.tensor(
                        [[64, 64, 0, 0, 64, 64]] * prompt.shape[0],
                        device=memory.device,
                        dtype=torch.float16,
                    )
                    with placements[2].using("reference", ("unet",)):
                        for time in schedule.timesteps:
                            batch = (
                                torch.cat([latent] * 2)
                                if payload.guidance > 1
                                else latent
                            )
                            prediction = components["unet"](
                                schedule.scale_model_input(batch, time),
                                time,
                                encoder_hidden_states=prompt,
                                added_cond_kwargs={
                                    "text_embeds": pooled,
                                    "time_ids": ids,
                                },
                            ).sample
                            if payload.guidance > 1:
                                low, high = prediction.chunk(2)
                                prediction = low + payload.guidance * (high - low)
                            latent = schedule.step(prediction, time, latent).prev_sample
                    vae = components["vae"]
                    with placements[3].using("reference", ("vae",)):
                        original = next(vae.parameters()).dtype
                        vae.to(dtype=wide)
                        try:
                            image = decode_latents(vae, latent.to(wide), 0.18215)
                        finally:
                            vae.to(dtype=original)
                    return latent, prompt, pooled, image

            expected_a = reference(tokens_a, payload.seed) if payload.mode == "phases" else None
            expected_b = reference(tokens_b, payload.seed + 1) if payload.mode == "phases" else None
            oracle_first = oracle_second = None
            torch.cuda.synchronize(memory.device)
            memory.trim()
            parameters = {
                "components": components,
                "scheduler_config": dict(scheduler.config),
                "vae_scale": 0.18215,
                "tokens": tokens_a,
                "negative_tokens": negative,
                "width": 64,
                "height": 64,
                "steps": run_steps,
                "guidance": payload.guidance,
                "ctx": ctx,
                "tel": tel,
            }
            if payload.mode == "prepare-refusal":
                failure = None
                try:
                    cache.prepare(**parameters)
                except RuntimeError as error:
                    failure = str(error)
                assert failure is not None and "64" in failure and "32" in failure, failure
                assert "retire executor" not in failure and cache._program is None
                gc.collect()
                assert memory.native.stats()["pinned_bytes"] == initial["pinned_bytes"]
                return Report(
                    True,
                    json.dumps({
                        "pid": os.getpid(),
                        "mode": "prepare-refusal",
                        "message": failure,
                        "pins_after_partial_close": memory.native.stats()["pinned_bytes"],
                        "scope": "actual incompatible VAE output shape refused after TE/UNet capture; no native sequence submission; next request checks healthy executor",
                    }, sort_keys=True),
                )
            assert cache.prepare(**parameters), (
                "sequential admitted preparation refused"
            )
            program = cache._program
            assert isinstance(program, _SequentialPipeline), (
                "resident composite fallback is not this proof"
            )
            assert program._interval.shared_pool
            assert program.plan._capture_domain is not None
            definition = id(program.plan)
            initial_calls = dict(program._interval.calls)
            results = []
            if payload.mode == "source-error":
                for chunk in private_chunks:
                    with chunk.open("wb"):
                        pass
                program.update(tokens_a, negative, payload.seed)
                execution = ctx.plan(program.plan)
                try:
                    execution.wait()
                except Exception as error:
                    facts = execution.facts()
                    assert facts["sequence"]["completion_proven"]
                    assert all(
                        row["completion_proven"] for row in facts["stage_inputs"]
                    )
                    assert "retire executor" not in str(error)
                    cache.close()
                    assert (
                        memory.native.stats()["pinned_bytes"] == initial["pinned_bytes"]
                    )
                    tel.log(
                        "PHASE_FAILURE "
                        + json.dumps(
                            {
                                "pid": os.getpid(),
                                "complete": True,
                                "pins": memory.native.stats()["pinned_bytes"],
                                "transitions": facts["native_phases"][0]["transitions"],
                            },
                            separators=(",", ":"),
                        )
                    )
                    raise
                raise AssertionError("private truncated phase source succeeded")
            if payload.mode == "cancel":
                program.update(tokens_a, negative, payload.seed)
                execution = ctx.plan(program.plan)
                deadline = time.monotonic() + 10
                while execution.facts()["sequence"]["completed_passes"] < 1:
                    assert time.monotonic() < deadline
                    time.sleep(0.001)
                execution.cancel()
                execution.wait()
                facts = execution.facts()
                assert (
                    facts["sequence"]["completion_proven"]
                    and facts["sequence"]["canceled"]
                )
                assert 1 <= facts["sequence"]["completed_passes"] < run_steps
                assert facts["native_phases"][0]["transitions"] == 2, facts[
                    "native_phases"
                ]
                cache.close()
                assert memory.native.stats()["pinned_bytes"] == initial["pinned_bytes"]
                return Report(
                    True,
                    json.dumps(
                        {
                            "pid": os.getpid(),
                            "mode": "cancel",
                            "facts": facts,
                            "scope": "phasewrapped source cancellation after native progress; finalVAE notentered",
                        },
                        sort_keys=True,
                    ),
                )
            old_output = old_copy = None
            for attempt, (tokens, seed, expected) in enumerate(
                (
                    (tokens_a, payload.seed, expected_a),
                    (tokens_b, payload.seed + 1, expected_b),
                )
            ):
                if attempt == 0:
                    program.update(tokens, negative, seed)
                    execution = ctx.plan(program.plan)
                    outputs = execution.wait()
                else:
                    # Exercise the actual existing cache's ordinary request path, including
                    # key/validity reuse and sequence-pass progress accounting.
                    image = cache.run(seed=seed, **{**parameters, "tokens": tokens})
                    assert image is not None and cache._program is program
                    execution = program.plan._active
                    outputs = (*program.plan.state[:3], image)
                errors = [
                    float((a.float() - b.float()).abs().max())
                    for a, b in zip(outputs, expected, strict=True)
                ]
                assert all(
                    torch.equal(a, b) for a, b in zip(outputs, expected, strict=True)
                ), errors
                assert id(cache._program.plan) == definition and program.plan.valid()
                assert program._interval.calls == initial_calls
                facts = execution.facts()
                phase_rows = facts.get("native_phases", [])
                assert len(phase_rows) == 1 and phase_rows[0]["transitions"] == 3, (
                    phase_rows
                )
                assert phase_rows[0]["active"].startswith("vae:") and len(phase_rows[0]["active"]) <= 193, phase_rows
                assert (
                    memory._phase_names == frozenset() and memory._phase_request is None
                )
                assert (
                    facts["sequence"]["completed_passes"] == payload.steps
                    and facts["sequence"]["completion_proven"]
                )
                assert (
                    len(facts["stage_inputs"])
                    == len(program._interval.definitions) - 1 + 2
                )
                if old_output is not None:
                    assert torch.equal(old_output, old_copy)
                    assert not torch.equal(old_output, outputs[-1])
                # The first ordinary consumer after native completion must recompute its
                # Python phase view, not reuse stale capture-order metadata.
                memory._activate_phase(models[0])
                assert memory._phase_names == request.phase(models[0].name)
                old_output, old_copy = outputs[-1], outputs[-1].clone()
                results.append(
                    {
                        "seed": seed,
                        "exact_errors": errors,
                        "facts": facts,
                        "native": memory.native.stats(),
                        "torch_allocated": torch.cuda.memory_allocated(memory.device),
                        "torch_reserved": torch.cuda.memory_reserved(memory.device),
                    }
                )
            return Report(
                True,
                json.dumps(
                    {
                        **versions(),
                        "pid": os.getpid(),
                        "steps": payload.steps,
                        "guidance": payload.guidance,
                        "variant": "sequential TE→streamed UNet→wide resident VAE",
                        "definition": definition,
                        "vae_dtype": str(wide),
                        "phase_capture_memory": program._capture_memory,
                        "interval": program._interval.facts(),
                        "results": results,
                        "scope": "real reduced fourcomponent SDXL fullnativephase sequence and stableinput reuse; no fullpretrainedcapacity orspeedclaim",
                    },
                    sort_keys=True,
                ),
            )
    finally:
        cache.close()
        for placement in placements:
            placement.release()
        for backend in backends:
            backend.close()
        gc.collect()
        memory.trim()
        memory.window_bytes = old_window
        memory.native.set_streaming_limit(old_window)
        after = memory.native.stats()
        assert (
            not after["poisoned"] and after["pinned_bytes"] == initial["pinned_bytes"]
        ), after
        for checkpoint in checkpoints:
            checkpoint.close()
