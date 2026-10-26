"""Real own-VA bounded streaming repeated entirely by the native sequence worker."""

from __future__ import annotations

import gc
import json
import os
import tempfile
from pathlib import Path

import msgspec
from cozy_runtime.author import Context, Telemetry


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "repeat"
    ordering: str = "events"
    seed: int = 2619311


class Report(msgspec.Struct):
    passed: bool
    detail: str


def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import tensorfs
    import torch
    from cozy_runtime.compute import PreparedRepeat, PreparedSequence
    from cozy_runtime.internal import accel, residency

    from .common import assert_slots, current_request, place, slots, versions

    assert payload.mode in {
        "repeat",
        "reuse",
        "warm",
        "stale",
        "region",
        "source",
        "escaped",
    }
    assert payload.ordering in {"events", "checked"}
    tel.log(
        "MODEL_STAGE "
        + json.dumps(
            {
                "pid": os.getpid(),
                "mode": payload.mode,
                "ordering": payload.ordering,
                "seed": payload.seed,
            }
        )
    )
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open(str(ctx.device))
    request = current_request(memory)
    initial = memory.native.stats()
    width, passes = 1024, 30
    factors = (0.5, 2.0, 1.0)
    cpu_weights = [torch.eye(width) * factor for factor in factors]

    class Toy(torch.nn.Sequential):
        _no_split_modules = ("Linear",)

        def __init__(self):
            super().__init__(
                *(torch.nn.Linear(width, width, bias=False) for _ in factors)
            )

    with tempfile.TemporaryDirectory(prefix="cozy-own-va-repeat-") as folder:
        writer = tensorfs.Writer(Path(folder) / "weights.cozytensors")
        for index, weight in enumerate(cpu_weights):
            writer.component("net").put(f"{index}.weight", weight)
        checkpoint = writer.commit()
        root, backend, placement = place(memory, checkpoint, "net", Toy)
        model = backend.models["net"]
        blocks = model.groups()
        assert len(blocks) == 3
        placement.open_attempt()
        placement.plan(("net",), request)
        model._claim()
        saved = slots(model)
        plans, calls = [], {}
        escaped = []
        program = None
        old_window = memory.window_bytes
        try:
            # Establish the actual phase before installing this explicit test window;
            # prepare_load must not overwrite the asserted policy on its first activation.
            memory._activate_phase(model)
            regions = [memory._native_region(block) for block in blocks]
            pointers = [region.ptr for region in regions]
            assert len(set(pointers)) == 3
            for block, region in zip(blocks, regions, strict=True):
                block.preferred = False
                block.preference_set = True
                region.set_preferred(False)
            window = max(region.allocated_bytes for region in regions)
            weight_bytes = sum(block.storage_bytes for block in blocks)
            assert weight_bytes == 12 * 1024**2 and window == 4 * 1024**2
            memory.native.set_streaming_limit(window)
            with torch.inference_mode(False):
                data = (
                    torch.arange(width, device=memory.device, dtype=torch.float32)
                    + payload.seed % 13
                ).reshape(1, width)
                state = torch.zeros_like(data)
                step = torch.zeros((1,), device=memory.device, dtype=torch.int64)
                output = torch.zeros_like(data)

            def eager_reference():
                # Independent eager streamed reference: one temporary GPU weight at a time.
                # This is outside either native submission, not part of a speed measurement.
                expected = data.clone()
                for outer in range(passes):
                    for cpu_weight in cpu_weights:
                        weight = cpu_weight.to(memory.device)
                        expected = torch.nn.functional.linear(expected, weight)
                        del weight
                    expected.add_(outer + 1)
                return expected.mul_(3)

            expected = eager_reference()
            torch.cuda.synchronize(memory.device)

            def resident(name, operation, state_tensors, owners=()):
                calls[name] = 0

                def body(index):
                    del index
                    calls[name] += 1
                    operation()

                plan = PreparedRepeat.capture(
                    body, state=state_tensors, steps=1, owners=owners
                )
                plans.append(plan)
                return plan

            def prefix():
                state.copy_(data)
                step.zero_()

            prefix_plan = resident("prefix", prefix, (state, step), (data,))
            block_plans = []
            for index, (block, region) in enumerate(zip(blocks, regions, strict=True)):
                name = f"block{index}"
                calls[name] = 0
                if payload.mode == "warm":
                    warm = model.retain((block.path,))
                    try:
                        with warm.prepare():
                            pass
                    finally:
                        warm.close()
                    assert region.resident

                def operation(index_tensor, name=name, index=index):
                    del index_tensor
                    calls[name] += 1
                    if payload.mode == "escaped":
                        escaped.append(root[index].weight)
                    state.copy_(root[index](state))

                try:
                    plan = PreparedRepeat.capture_stage(
                        operation,
                        state=(state,),
                        owner=root,
                        paths=(block.path,),
                        load_ordering=payload.ordering,
                    )
                except Exception as error:
                    if payload.mode != "escaped":
                        raise
                    from cozy_runtime.author import CapabilityError

                    assert (
                        isinstance(error, CapabilityError)
                        and error.code == "prepared_graph_ineligible"
                    ), error
                    assert escaped and index == 0
                    assert_slots(saved)
                    assert not state.count_nonzero().item()
                    return Report(
                        True,
                        json.dumps(
                            {
                                **versions(),
                                "pid": os.getpid(),
                                "mode": "escaped",
                                "refused": True,
                                "reason": str(error),
                            },
                            sort_keys=True,
                        ),
                    )
                plans.append(plan)
                block_plans.append(plan)
                assert_slots(saved)
                assert not state.count_nonzero().item() and not step.item()
                snapshot = memory.native.stats()
                assert snapshot["pinned_bytes"] == initial["pinned_bytes"], snapshot
                assert (
                    snapshot["resident_bytes"] - initial["resident_bytes"] <= window
                ), snapshot

            def sample():
                state.add_(step.to(torch.float32).reshape(()) + 1)
                step.add_(1)

            def suffix():
                output.copy_(state * 3)

            sample_plan = resident("sampler", sample, (state, step))
            suffix_plan = resident("suffix", suffix, (output,), (state,))
            program = PreparedSequence(
                (prefix_plan, *block_plans, sample_plan, suffix_plan),
                repeat=(1, 5, passes),
            )
            if payload.mode in {"stale", "region", "source"}:
                import copy

                generation, source, region = (
                    model.generation,
                    model.source,
                    blocks[0].region,
                )
                if payload.mode == "stale":
                    model.generation += 1
                elif payload.mode == "region":
                    blocks[0].region = None
                else:
                    model.source = copy.copy(source)
                try:
                    assert not program.valid()
                    try:
                        program.submit()
                    except RuntimeError as error:
                        assert "binding" in str(error) or "fill" in str(error), error
                    else:
                        raise AssertionError("changed model generation was accepted")
                    assert all(
                        plan.facts()["host_graph_launches"] == 0 for plan in plans
                    )
                finally:
                    model.generation, model.source, blocks[0].region = (
                        generation,
                        source,
                        region,
                    )
                return Report(
                    True,
                    json.dumps(
                        {
                            **versions(),
                            "pid": os.getpid(),
                            "mode": payload.mode,
                            "refused": True,
                            "zero_launches": True,
                        },
                        sort_keys=True,
                    ),
                )
            before_calls = dict(calls)
            snapshots = []
            previous = previous_snapshot = None
            submissions = 2 if payload.mode == "reuse" else 1
            for submission in range(submissions):
                if submission:
                    data.add_(7)
                    expected = eager_reference()
                    torch.cuda.synchronize(memory.device)
                execution = ctx.plan(program)
                (actual,) = execution.wait()
                assert torch.equal(actual, expected), float(
                    (actual - expected).abs().max()
                )
                assert step.item() == passes
                assert calls == before_calls, "Python body ran after native submission"
                assert_slots(saved)
                assert [region.ptr for region in regions] == pointers
                facts = execution.facts()
                inputs = facts["stage_inputs"]
                assert [row["stage"] for row in inputs] == [1, 2, 3], inputs
                assert [row["read_bytes"] for row in inputs] == [
                    block.storage_bytes * passes for block in blocks
                ], inputs
                assert all(
                    row["completion_proven"]
                    and row["passes_started"] == passes
                    and row["completed_reads"] == passes
                    for row in inputs
                ), inputs
                assert facts["sequence"]["completed_passes"] == passes, facts
                assert facts["sequence"]["completed_stages"] == 4 * passes + 2, facts
                assert facts["host_graph_launches"] == (submission + 1) * (
                    4 * passes + 2
                ), facts
                assert prefix_plan.facts()["host_graph_launches"] == submission + 1
                assert suffix_plan.facts()["host_graph_launches"] == submission + 1
                after = memory.native.stats()
                assert after["pinned_bytes"] == initial["pinned_bytes"], after
                assert after["resident_bytes"] - initial["resident_bytes"] <= window, (
                    after
                )
                if previous is not None:
                    assert torch.equal(previous, previous_snapshot), (
                        "reuse mutated prior output"
                    )
                    assert not torch.equal(previous, actual), (
                        "changed input reused old values"
                    )
                previous, previous_snapshot = actual, actual.clone()
                snapshots.append(
                    {
                        "submission": submission,
                        "facts": facts,
                        "after": after,
                        "gpu_step": step.item(),
                        "exact_eager_output": True,
                    }
                )
            return Report(
                True,
                json.dumps(
                    {
                        **versions(),
                        "pid": os.getpid(),
                        "mode": payload.mode,
                        "ordering": payload.ordering,
                        "seed": payload.seed,
                        "calls": calls,
                        "window_bytes": window,
                        "weight_bytes": weight_bytes,
                        "pointers": pointers,
                        "submissions": snapshots,
                        "scope": "actual placed-module capture_stage adapter30-pass streaming; same-request reuse only; not a diffusion benchmark",
                    },
                    sort_keys=True,
                ),
            )
        finally:
            if program is not None:
                program.close()
            for plan in plans:
                plan.close()
            escaped.clear()
            gc.collect()
            model._release()
            placement.release()
            backend.close()
            memory.trim()
            memory.native.set_streaming_limit(old_window)
            cleanup = memory.native.stats()
            assert (
                not cleanup["poisoned"]
                and cleanup["pinned_bytes"] == initial["pinned_bytes"]
            ), cleanup
            checkpoint.close()
