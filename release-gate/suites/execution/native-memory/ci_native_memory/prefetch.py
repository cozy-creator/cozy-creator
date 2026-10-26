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
    mode: str = "overlap"
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

    assert payload.mode in {"overlap", "capacity", "reuse", "cancel"}
    assert payload.ordering in {"events", "checked"}
    tel.log(
        "STAGE_PREFETCH "
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
    width, passes, inner = 1024, 1000 if payload.mode == "cancel" else 4, 32
    factors = (1.0, 1.0, 1.0)
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
        program = None
        old_window = memory.window_bytes
        old_budget = memory.budget_bytes
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
            memory.window_bytes = window
            with torch.inference_mode(False):
                data = (
                    (
                        torch.arange(width, device=memory.device, dtype=torch.float32)
                        + payload.seed % 13
                    )
                    .reshape(1, width)
                    .expand(512, -1)
                    .clone()
                )
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

            expected = None if payload.mode == "cancel" else eager_reference()
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
            for index, block in enumerate(blocks):
                name = f"block{index}"
                calls[name] = 0

                def operation(index_tensor, name=name, index=index):
                    del index_tensor
                    calls[name] += 1
                    for _ in range(inner):
                        state.copy_(root[index](state))

                plan = PreparedRepeat.capture_stage(
                    operation,
                    state=(state,),
                    owner=root,
                    paths=(block.path,),
                    load_ordering=payload.ordering,
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
            before_calls = dict(calls)
            snapshots = []
            previous = previous_snapshot = None
            submissions = 2 if payload.mode == "reuse" else 1
            for submission in range(submissions):
                if submission:
                    data.add_(7)
                    expected = eager_reference()
                    torch.cuda.synchronize(memory.device)
                if payload.mode == "capacity":
                    # A real tighter request budget leaves no speculative room while demand
                    # can keep its existing single-region window through ordinary reclamation.
                    memory.budget_bytes = memory._figures()[1]
                execution = ctx.plan(program)
                if payload.mode == "cancel":
                    deadline = time.monotonic() + 20
                    while not any(
                        row.get("prefetch_started", 0)
                        for row in execution.facts()["stage_inputs"]
                    ):
                        if time.monotonic() >= deadline:
                            raise TimeoutError("native lookahead never started")
                        time.sleep(0.001)
                    execution.cancel()
                (actual,) = execution.wait()
                if payload.mode != "cancel":
                    assert torch.equal(actual, expected), float(
                        (actual - expected).abs().max()
                    )
                    assert step.item() == passes
                assert calls == before_calls, "Python body ran after native submission"
                assert_slots(saved)
                assert [region.ptr for region in regions] == pointers
                facts = execution.facts()
                admission = facts["stage_prefetch_admission"]
                if payload.mode == "capacity":
                    assert admission and all(
                        row["state"] == "capacity_skip" for row in admission
                    ), admission
                else:
                    links = [row for row in admission if row["state"] == "linked"]
                    assert [(row["from_stage"], row["to_stage"]) for row in links] == [
                        (1, 2),
                        (2, 3),
                    ], admission
                assert memory.window_bytes == window
                assert memory.native.stats()["streaming_limit_bytes"] == window
                inputs = facts["stage_inputs"]
                assert [row["stage"] for row in inputs] == [1, 2, 3], inputs
                speculative = sum(row.get("prefetch_read_bytes", 0) for row in inputs)
                if payload.mode == "capacity":
                    assert speculative == 0
                elif submission and all(row["read_bytes"] == 0 for row in inputs):
                    # The first submission retained all fitting weights. Reuse must not
                    # manufacture traffic solely to exercise the prefetch optimization.
                    assert speculative == 0
                    assert all(row.get("prefetch_started", 0) == 0 for row in inputs), (
                        inputs
                    )
                    assert all(
                        row.get("prefetch_issued_bytes", 0) == 0 for row in inputs
                    ), inputs
                elif payload.mode != "cancel":
                    assert speculative > 0, inputs
                    # A following source can already be cached; preserving that useful
                    # cache is valid and must not force a redundant speculative read.
                    assert any(
                        row.get("prefetch_started", 0) > 0 for row in inputs[1:]
                    ), inputs
                assert all(row["completion_proven"] for row in inputs), inputs
                if payload.mode != "cancel":
                    assert all(
                        row["passes_started"] == passes
                        and row["completed_reads"] == passes
                        for row in inputs
                    ), inputs
                if payload.mode == "cancel":
                    assert (
                        facts["sequence"]["canceled"]
                        and facts["sequence"]["completion_proven"]
                    ), facts
                    assert facts["sequence"]["completed_passes"] < passes, facts
                    assert suffix_plan.facts()["host_graph_launches"] == 0
                else:
                    assert facts["sequence"]["completed_passes"] == passes, facts
                    assert facts["sequence"]["completed_stages"] == 4 * passes + 2, (
                        facts
                    )
                if payload.mode != "cancel":
                    assert facts["host_graph_launches"] == (submission + 1) * (
                        4 * passes + 2
                    ), facts
                    assert suffix_plan.facts()["host_graph_launches"] == submission + 1
                assert prefix_plan.facts()["host_graph_launches"] == submission + 1
                after = memory.native.stats()
                assert after["pinned_bytes"] == initial["pinned_bytes"], after
                # Native streaming_limit bounds idle cache, not active pins. Real admitted
                # capacity may preserve useful old blocks; observe actual residency honestly.
                assert after["resident_bytes"] - initial["resident_bytes"] <= sum(
                    region.allocated_bytes for region in regions
                ), after
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
                        "exact_eager_output": payload.mode != "cancel",
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
                        "scope": (
                            "adjacent-source native prefetch admission; actual overlap needs "
                            "native event timestamps; not a diffusion benchmark"
                        ),
                    },
                    sort_keys=True,
                ),
            )
        finally:
            if program is not None:
                program.close()
            for plan in plans:
                plan.close()
            gc.collect()
            model._release()
            placement.release()
            backend.close()
            memory.trim()
            memory.budget_bytes = old_budget
            memory.window_bytes = old_window
            memory.native.set_streaming_limit(old_window)
            cleanup = memory.native.stats()
            assert (
                not cleanup["poisoned"]
                and cleanup["pinned_bytes"] == initial["pinned_bytes"]
            ), cleanup
            checkpoint.close()
