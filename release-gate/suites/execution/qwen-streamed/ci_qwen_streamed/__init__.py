"""Real placed Diffusers Qwen21 blocks, immutable sources and native cached steps."""

from __future__ import annotations

import gc
import json
import os
import shutil
import tempfile
import time
from functools import partial
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    lane: str = "bf16"
    mode: str = "repeat"
    seed: int = 3301001


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import tensorfs
    import torch
    from cozy_runtime import graphs
    from cozy_runtime.internal import accel, int8_linear, kernel_ready, residency
    from diffusers import FlowMatchEulerDiscreteScheduler, QwenImage21Transformer2DModel
    from diffusers.models.transformers.transformer_qwenimage21 import QwenImage21KVCache

    from ._prepared import ResidentDenoise, _FlowEuler, _Layout, _StreamedDenoise
    from .common import assert_slots, current_request, place, slots, versions

    assert payload.lane in {"bf16", "int8"}
    assert payload.mode in {
        "repeat",
        "reuse",
        "identity",
        "cancel",
        "pressure",
        "selection",
        "not-local",
    }
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(str(ctx.device))
    request = current_request(memory)
    if payload.lane == "int8":
        device_index = memory.device.index or 0
        kernel_ready.wait(
            f"{int8_linear.NATIVE} cuda:{device_index}",
            partial(int8_linear._library, torch, device_index),
        )
        assert int8_linear.native(torch, device_index) is not None
        assert int8_linear.serving(device_index) == int8_linear.OP
    config = {
        "in_channels": 64,
        "out_channels": 64,
        "num_layers": 3,
        "num_attention_heads": 2,
        "attention_head_dim": 128,
        "context_in_dim": 256,
        "axes_dims_rope": (16, 56, 56),
        "causal_condition": True,
    }

    def factory():
        return QwenImage21Transformer2DModel(**config).to(torch.bfloat16).eval()

    with torch.random.fork_rng(devices=[]):
        torch.manual_seed(payload.seed)
        source = factory()
    initial = memory.native.stats()
    program = None
    old_window, old_budget = memory.window_bytes, memory.budget_bytes
    with tempfile.TemporaryDirectory(prefix="cozy-qwen-streamed-") as folder:
        writer = tensorfs.Writer(Path(folder) / "weights.cozytensors")
        encoded = 0
        for key, value in source.state_dict().items():
            if (
                payload.lane == "int8"
                and key.startswith("transformer_blocks.")
                and value.ndim == 2
                and key.endswith(".weight")
            ):
                result = tensorfs.raw.quantize(
                    "int8.convrot",
                    value.float().numpy().tobytes(),
                    dtype="BF16",
                    shape=tuple(value.shape),
                    strategy="int8-convrot256/1",
                    params={"rotation": "kron_regular_h4", "rotation_size": 256},
                )
                assert result.parts["scale"].dtype == "F32"
                writer.component("transformer").put_encoded(
                    key,
                    dtype="BF16",
                    shape=tuple(value.shape),
                    scheme=result.scheme,
                    params=result.params,
                    parts={
                        role: (part.dtype, part.shape, part.tobytes())
                        for role, part in result.parts.items()
                    },
                )
                encoded += 1
            else:
                writer.component("transformer").put(key, value)
        checkpoint = writer.commit()
        private_chunks = []
        if payload.mode == "not-local":
            # Never remove shared CAS bytes: export and copy this generated checkpoint.
            exported = Path(tensorfs.materialize(checkpoint, local_dir=Path(folder) / "export"))
            private = Path(folder) / "private-input"
            shutil.copytree(exported, private, copy_function=shutil.copyfile, symlinks=False)
            checkpoint = tensorfs.open(
                str(private), local_files_only=True, cache_dir=Path(folder) / "private-cache"
            )
            private_chunks = [
                private / "chunks" / digest.removeprefix("sha256:")
                for digest, _ in checkpoint.chunks
            ]
            assert private_chunks and all(
                chunk.is_file() and chunk.stat().st_nlink == 1 for chunk in private_chunks
            )
        del source
        root, backend, placement = place(
            memory, checkpoint, "transformer", factory, encoded=payload.lane == "int8"
        )
        model = backend.models["transformer"]
        placement.open_attempt()
        placement.plan(("transformer",), request)
        model._claim()
        saved = slots(model)
        groups = model.groups()
        blocks = [block for block in groups if block.path]
        assert len(blocks) == 3 and all(
            block.path.startswith("transformer_blocks.") for block in blocks
        )
        assert all(block.aliases is not None and not block.finals for block in groups)
        regions = [memory._native_region(block) for block in groups]
        pointers = [region.ptr for region in regions]
        common = sum(
            region.allocated_bytes
            for block, region in zip(groups, regions)
            if not block.path
        )
        window = common + max(
            region.allocated_bytes
            for block, region in zip(groups, regions)
            if block.path
        )
        assert sum(region.allocated_bytes for region in regions) > window
        passes = 999 if payload.mode == "cancel" else 29
        try:
            attempt_contexts = []

            def attempt(seed):
                schedule = FlowMatchEulerDiscreteScheduler()
                schedule.set_timesteps(passes + 1, device=memory.device)
                schedule.set_begin_index(0)
                generator = torch.Generator(device=memory.device).manual_seed(seed)
                latent = torch.randn(
                    (1, 64, 64),
                    generator=generator,
                    dtype=torch.bfloat16,
                    device=memory.device,
                )
                embeds = torch.randn(
                    (1, 16, 256),
                    generator=generator,
                    dtype=torch.bfloat16,
                    device=memory.device,
                )
                img_mask = torch.cat(
                    [
                        torch.zeros((1, 16), dtype=torch.bool, device=memory.device),
                        torch.ones((1, 16), dtype=torch.bool, device=memory.device),
                    ],
                    dim=1,
                )
                shapes = [[(1, 8, 8)]]
                layout = _Layout(root, shapes, img_mask, None, memory.device)
                cache = QwenImage21KVCache(3)
                bypass = graphs.BYPASS.set(True)
                try:
                    with (
                        placement.using("qwen-streamed-oracle", ("transformer",)),
                        torch.inference_mode(False),
                        torch.no_grad(),
                    ):
                        first = root(
                            hidden_states=latent,
                            timestep=schedule.timesteps[0:1].to(torch.bfloat16) / 1000,
                            encoder_hidden_states=embeds,
                            img_shapes=shapes,
                            img_mask=img_mask,
                            kv_cache=cache,
                            kv_cache_mode="extract",
                            return_dict=False,
                        )[0][:, -64:]
                        latent = schedule.step(
                            first, schedule.timesteps[0], latent, return_dict=False
                        )[0]
                        expected = latent.clone()
                        if payload.mode != "cancel":
                            for timestep in schedule.timesteps[1:]:
                                noise = root(
                                    hidden_states=expected,
                                    timestep=timestep.reshape(1).to(torch.bfloat16)
                                    / 1000,
                                    encoder_hidden_states=embeds,
                                    img_shapes=shapes,
                                    img_mask=img_mask,
                                    kv_cache=cache,
                                    kv_cache_mode="cached",
                                    return_dict=False,
                                )[0][:, -64:]
                                expected = schedule.step(
                                    noise, timestep, expected, return_dict=False
                                )[0]
                finally:
                    graphs.BYPASS.reset(bypass)
                euler = _FlowEuler(schedule, memory.device, 1)
                caches = [cache.get_layer(i) for i in range(3)]
                inputs = (
                    latent,
                    euler.ts,
                    euler.dt,
                    layout.rotary_emb,
                    layout.modulation_mask,
                    *(tensor for row in caches for tensor in row.get()),
                )
                attempt_contexts.append((schedule, cache, embeds, img_mask, shapes))
                return inputs, euler, caches, layout, expected

            attempts = [attempt(payload.seed)]
            if payload.mode in {"reuse", "pressure"}:
                attempts.append(attempt(payload.seed + 1))
            torch.cuda.synchronize(memory.device)
            if payload.lane == "int8":
                assert int8_linear.serving(device_index) == int8_linear.OP
            with memory.lock:
                memory._settle_prefetch(cancel=True)
            memory._activate_phase(model)
            # Ordinary eager prefetch may replace an idle reservation when it settles.
            # Freeze the graph's actual reservations only after that preflight has ended.
            regions = [memory._native_region(block) for block in groups]
            pointers = [region.ptr for region in regions]
            for block, region in zip(groups, regions):
                block.preferred = False
                block.preference_set = True
                region.set_preferred(False)
            memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
            memory.window_bytes = window
            memory.native.set_streaming_limit(window)
            values, euler, caches, layout, expected = attempts[0]
            parents = values[0].clone()
            if payload.mode == "not-local":
                # The initial extract and oracle have finished. Make ONLY this private
                # source unavailable before any native source job can be accepted.
                hidden_chunks = []
                selector = ResidentDenoise()
                schedule, cache, embeds, img_mask, shapes = attempt_contexts[0]
                try:
                    for chunk in private_chunks:
                        hidden = chunk.with_name(chunk.name + ".held")
                        chunk.rename(hidden)
                        hidden_chunks.append((chunk, hidden))
                    assert all(not chunk.exists() for chunk in private_chunks)
                    assert checkpoint.missing(), "private source fell back to cached bytes"
                    binding = model.retain(())
                    try:
                        try:
                            binding.prepare_load(stage=True)
                        except RuntimeError as error:
                            assert getattr(error, "load_unavailable", False), str(error)
                            assert "not all local" in str(error), str(error)
                        else:
                            raise AssertionError("missing private source was accepted")
                    finally:
                        binding.close()
                    actual = selector.run(
                        transformer=root,
                        scheduler=schedule,
                        latents=values[0],
                        kv_cache=cache,
                        layout=layout,
                        ctx=ctx,
                        tel=tel,
                    )
                    assert actual is None and selector._program is None, (
                        "not-local preparation did not return the eager boundary"
                    )
                    assert torch.equal(values[0], parents), "preparation changed the input"
                    assert_slots(saved)
                    assert memory.native.stats()["pinned_bytes"] == initial["pinned_bytes"]
                finally:
                    selector.close()
                    for chunk, hidden in hidden_chunks:
                        hidden.rename(chunk)
                # Exercise the existing ordinary continuation after that refusal. Native
                # construction and accepted execution are never monkeypatched or retried.
                eager = values[0].clone()
                # The oracle loop left this attempt's scheduler at its last step: continue
                # from step 1 on a fresh one with the same timesteps.
                schedule = FlowMatchEulerDiscreteScheduler()
                schedule.set_timesteps(passes + 1, device=memory.device)
                schedule.set_begin_index(1)
                bypass = graphs.BYPASS.set(True)
                try:
                    with placement.using("qwen-not-local-fallback", ("transformer",)), torch.no_grad():
                        for timestep in schedule.timesteps[1:]:
                            noise = root(
                                hidden_states=eager,
                                timestep=timestep.reshape(1).to(torch.bfloat16) / 1000,
                                encoder_hidden_states=embeds,
                                img_shapes=shapes,
                                img_mask=img_mask,
                                kv_cache=cache,
                                kv_cache_mode="cached",
                                return_dict=False,
                            )[0][:, -64:]
                            eager = schedule.step(noise, timestep, eager, return_dict=False)[0]
                finally:
                    graphs.BYPASS.reset(bypass)
                assert torch.equal(eager, expected)
                assert not memory.native.stats()["poisoned"]
                assert_slots(saved)
                return Report(True, json.dumps({
                    **versions(), "pid": os.getpid(), "mode": payload.mode, "lane": payload.lane,
                    "pre_submission_refused": True, "eager_continuation_exact": True,
                    "private_chunks_restored": len(hidden_chunks),
                    "scope": "actual not-local native preparation refusal; restored private source then ordinary eager continuation; no remote fetch or post-submit retry claim",
                }))
            if payload.mode == "selection":
                selector = ResidentDenoise()
                schedule, cache, *_ = attempt_contexts[0]
                actual = selector.run(
                    transformer=root,
                    scheduler=schedule,
                    latents=values[0],
                    kv_cache=cache,
                    layout=layout,
                    ctx=ctx,
                    tel=tel,
                )
                program = selector._program
                assert isinstance(program, _StreamedDenoise), (
                    "resident refusal did not select native block streaming"
                )
                assert actual is not None and torch.equal(actual, expected)
                assert torch.equal(values[0], parents)
                facts = program.plan.facts()
                assert facts["host_graph_launches"] == 1 + passes * (len(blocks) + 2), (
                    facts
                )
                assert_slots(saved)
                assert memory.native.stats()["pinned_bytes"] == initial["pinned_bytes"]
                if payload.lane == "int8":
                    assert int8_linear.serving(device_index) == int8_linear.OP
                return Report(
                    True,
                    json.dumps(
                        {
                            **versions(),
                            "pid": os.getpid(),
                            "mode": payload.mode,
                            "lane": payload.lane,
                            "streamed_selected": True,
                            "native_cached_passes": passes,
                            "host_graph_launches": facts["host_graph_launches"],
                            "exact_eager_output": True,
                            "scope": "actual package resident-refusal routing into streamed cached interval; no full pretrained image claim",
                        }
                    ),
                )
            program = _StreamedDenoise(
                ("real-qwen-gate",), root, values, euler, caches, layout, passes + 1
            )
            # This pressure cell intentionally chooses cold demand streaming. Separate final
            # cohort prefetch cells cover speculation and useful warm-cache retention.
            program.plan.prefetch = False
            if payload.lane == "int8":
                assert int8_linear.serving(device_index) == int8_linear.OP
            if payload.mode == "identity":
                generation = model.generation
                model.generation += 1
                try:
                    assert not program.plan.valid()
                    try:
                        program.plan.submit()
                    except RuntimeError as error:
                        assert "binding" in str(error) or "fill" in str(error)
                    else:
                        raise AssertionError("changed model identity accepted")
                    assert program.plan.facts()["host_graph_launches"] == 0
                finally:
                    model.generation = generation
                return Report(
                    True,
                    json.dumps(
                        {
                            **versions(),
                            "pid": os.getpid(),
                            "mode": payload.mode,
                            "refused": True,
                        }
                    ),
                )
            records, previous, previous_copy = [], None, None
            competitor = None
            for number, (values, _, _, _, expected) in enumerate(attempts):
                if number and payload.mode == "pressure":
                    competitor = torch.empty(
                        (128 * 1024**2,), dtype=torch.uint8, device=memory.device
                    )
                    competitor.fill_(71)
                memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
                assert all(not region.resident for region in regions)
                program.update(values)
                execution = ctx.plan(program.plan)
                if payload.mode == "cancel":
                    deadline = time.monotonic() + 30
                    while execution.facts()["sequence"]["completed_passes"] < 2:
                        assert time.monotonic() < deadline
                        time.sleep(0.001)
                    execution.cancel()
                (actual,) = execution.wait()
                facts = execution.facts()
                assert facts["sequence"]["completion_proven"]
                if payload.mode == "cancel":
                    assert (
                        facts["sequence"]["canceled"]
                        and 0 < facts["sequence"]["completed_passes"] < passes
                    )
                else:
                    assert torch.equal(actual, expected), float(
                        (actual.float() - expected.float()).abs().max()
                    )
                    assert facts["sequence"]["completed_passes"] == passes
                assert_slots(saved)
                assert [region.ptr for region in regions] == pointers
                after = memory.native.stats()
                assert (
                    after["pinned_bytes"] == initial["pinned_bytes"]
                    and not after["poisoned"]
                )
                # The streaming limit bounds idle cache; Runtime may keep one admitted next
                # source resident beside it (Memory._stage_prefetch_window).
                grown = after["resident_bytes"] - initial["resident_bytes"]
                largest = max(r.allocated_bytes for k, r in zip(groups, regions) if k.path)
                assert grown <= window + largest, (grown, window, largest)
                read = sum(row["read_bytes"] for row in facts["stage_inputs"])
                assert read > sum(block.storage_bytes for block in groups), read
                assert all(row["completion_proven"] for row in facts["stage_inputs"])
                if previous is not None:
                    assert torch.equal(previous, previous_copy)
                previous, previous_copy = actual, actual.clone()
                records.append(
                    {
                        "facts": facts,
                        "after": after,
                        "exact": payload.mode != "cancel",
                        "competitor_bytes": 0
                        if competitor is None
                        else competitor.numel(),
                    }
                )
            assert torch.equal(attempts[0][0][0], parents)
            if payload.lane == "int8":
                assert (
                    encoded == 21
                    and int8_linear.serving(device_index) == int8_linear.OP
                )
            return Report(
                True,
                json.dumps(
                    {
                        **versions(),
                        "pid": os.getpid(),
                        "lane": payload.lane,
                        "mode": payload.mode,
                        "encoded_weights": encoded,
                        "weight_bytes": sum(block.storage_bytes for block in groups),
                        "window": window,
                        "submissions": records,
                        "scope": "actual reduced Qwen21 cached interval with real native source block refills; eager extract0 retained; no pretrained image or speed claim",
                    }
                ),
            )
        finally:
            if program is not None:
                program.plan.close()
            gc.collect()
            model._release()
            placement.release()
            backend.close()
            memory.trim()
            memory.window_bytes, memory.budget_bytes = old_window, old_budget
            memory.native.set_streaming_limit(old_window)
            checkpoint.close()
