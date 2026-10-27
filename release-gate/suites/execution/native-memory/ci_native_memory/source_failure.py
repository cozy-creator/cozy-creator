"""Real own-VA bounded streaming repeated entirely by the native sequence worker."""

from __future__ import annotations

import gc
import json
import os
import tempfile
import time
from pathlib import Path

import msgspec
from cozy_runtime.author import Context, Telemetry


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "cancel"
    ordering: str = "events"
    seed: int = 2620011


class Report(msgspec.Struct):
    passed: bool
    detail: str


def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import tensorfs
    import torch
    from cozy_runtime.compute import PreparedRepeat, PreparedSequence
    from cozy_runtime.internal import accel, residency
    from cozy_runtime.internal.raw_stage import capture_raw_stage

    from .common import (
        assert_slots,
        current_request,
        place,
        private_input,
        slots,
        versions,
    )

    assert payload.mode in {"repeat", "cancel", "source-error"}
    assert payload.ordering in {"events", "checked"}
    tel.log(
        "STAGE_FAILURE "
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
    width, passes = 1024, (100_000 if payload.mode == "cancel" else 30)
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
        chunks = []
        if payload.mode == "source-error":
            private, chunks = private_input(checkpoint, Path(folder))
            checkpoint.close()
            checkpoint = private
        _root, backend, placement = place(memory, checkpoint, "net", Toy)
        model = backend.models["net"]
        blocks = model.groups()
        assert len(blocks) == 3
        placement.open_attempt()
        placement.plan(("net",), request)
        model._claim()
        saved = slots(model)
        plans, calls = [], {}
        program = None
        expected_failure = None
        old_window = memory.window_bytes
        try:
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

            expected = data.clone() if payload.mode == "cancel" else eager_reference()
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
                offset, parts = 0, []
                for key, roles in block.parts.items():
                    for part in roles.values():
                        parts.append(
                            (f"net/{key}#{part.role or 'value'}", offset, part.nbytes)
                        )
                        offset += (part.nbytes + 255) // 256 * 256
                assert offset == block.storage_bytes
                name = f"block{index}"
                calls[name] = 0

                def operation(index_tensor, inputs, name=name):
                    del index_tensor
                    calls[name] += 1
                    weight = inputs[0].view(torch.float32).reshape(width, width)
                    state.copy_(torch.nn.functional.linear(state, weight))

                plan = capture_raw_stage(
                    checkpoint,
                    [(region, parts)],
                    operation,
                    state=(state,),
                    ordering=payload.ordering,
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
                # This gate intentionally proves cold demand streaming at one weight window.
                # Speculation and warm ready-phase retention have separate cells.
                prefetch=False,
            )
            memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
            assert all(not region.resident for region in regions)
            memory.window_bytes = window
            memory.native.set_streaming_limit(window)
            before_calls = dict(calls)
            if payload.mode == "source-error":
                # Descriptor preflight has opened every private chunk. Truncate only
                # fixture-owned copies, so the submitted native read itself fails.
                assert chunks
                for chunk in chunks:
                    with chunk.open("wb"):
                        pass
                execution = ctx.plan(program)
                try:
                    execution.wait()
                except Exception as error:
                    facts = execution.facts()
                    assert (
                        facts["sequence"]["done"]
                        and facts["sequence"]["completion_proven"]
                    ), facts
                    assert not program.valid(), (
                        "failed source definition remained reusable"
                    )
                    assert all(
                        row["completion_proven"] for row in facts["stage_inputs"]
                    ), facts
                    after = memory.native.stats()
                    assert (
                        not after["poisoned"]
                        and after["pinned_bytes"] == initial["pinned_bytes"]
                    ), after
                    assert (
                        "retire executor" not in str(error)
                        and getattr(error, "error_code", None) is None
                    )
                    expected_failure = str(error)
                    event = "STAGE_FAILURE " + json.dumps(
                        {
                            "pid": os.getpid(),
                            "event": "read-failed-cleanly",
                            "completion": facts["sequence"]["completion_proven"],
                            "pins": after["pinned_bytes"],
                            "no_launch": all(
                                stage.facts()["host_graph_launches"] == 0
                                for stage in block_plans
                            ),
                        },
                        separators=(",", ":"),
                    )
                    assert len(event) <= 190, event
                    tel.log(event)
                    raise
                raise AssertionError("truncated private source unexpectedly succeeded")
            if payload.mode == "cancel":
                execution = ctx.plan(program)
                deadline = time.monotonic() + 10
                while execution.facts()["sequence"]["completed_passes"] < 2:
                    assert time.monotonic() < deadline, execution.facts()
                    time.sleep(0.001)
                execution.cancel()
                execution.wait()
                facts = execution.facts()
                assert (
                    facts["sequence"]["canceled"]
                    and facts["sequence"]["completion_proven"]
                ), facts
                assert 0 < facts["sequence"]["completed_passes"] < passes
                assert step.item() == facts["sequence"]["completed_passes"], facts
                assert suffix_plan.facts()["host_graph_launches"] == 0
                assert calls == before_calls
                assert all(row["completion_proven"] for row in facts["stage_inputs"]), (
                    facts
                )
                after = memory.native.stats()
                assert (
                    not after["poisoned"]
                    and after["pinned_bytes"] == initial["pinned_bytes"]
                ), after
                return Report(
                    True,
                    json.dumps(
                        {
                            **versions(),
                            "pid": os.getpid(),
                            "mode": payload.mode,
                            "ordering": payload.ordering,
                            "facts": facts,
                            "after": after,
                            "calls": calls,
                            "gpu_step": step.item(),
                            "scope": "real composed source-stage cancellation after native repeated progress; no guaranteed mid-copy cancellation claim",
                        },
                        sort_keys=True,
                    ),
                )
            snapshots = []
            previous = previous_snapshot = None
            submissions = 1
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
                        "scope": "real30-pass own-VA native streaming; reuse is a second submission in the same admitted request; not a production diffusion model or performance benchmark",
                    },
                    sort_keys=True,
                ),
            )
        finally:
            if program is not None:
                try:
                    program.close()
                except Exception as error:
                    # close may repeat the already-observed request-local source failure.
                    # A different cleanup error remains fatal; never suppress unproven work.
                    if expected_failure is None or str(error) != expected_failure:
                        raise
            for plan in plans:
                plan.close()
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
