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

    from .common import (
        assert_slots,
        current_request,
        place,
        private_input,
        slots,
        versions,
    )

    assert payload.mode in {
        "repeat",
        "reuse",
        "warm",
        "stale",
        "region",
        "source",
        "escaped",
        "unselected",
        "alias_write",
        "mutable_buffer",
        "sourceerror",
    }
    assert payload.ordering in {"events", "checked"}
    tel.log(
        "STORED_ROLE_STAGE "
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
    # Establish the exact native consumer before module materialization can start a
    # background GPU kernel probe beside global graph capture. This is a consumer gate,
    # not evidence that the production capture/probe race has been repaired.
    from functools import partial

    from cozy_runtime.internal import int8_linear, kernel_ready

    device_index = torch.device(str(ctx.device)).index or 0
    kernel_ready.wait(
        f"{int8_linear.NATIVE} cuda:{device_index}",
        partial(int8_linear._library, torch, device_index),
    )
    assert int8_linear.native(torch, device_index) is not None
    assert int8_linear.serving(device_index) == int8_linear.OP
    tel.log(
        "STORED_ROLE_NATIVE "
        + json.dumps(
            {
                "kernel": int8_linear.serving(device_index),
                "pending": len(kernel_ready.pending()),
            }
        )
    )
    memory = residency.Memory.open(str(ctx.device))
    request = current_request(memory)
    initial = memory.native.stats()
    width, passes = 1024, 30
    factors = (0.5, 2.0, 1.0)
    cpu_weights = [torch.eye(width) * factor for factor in factors]

    class Toy(torch.nn.Sequential):
        # Placement discovers blocks after encoded-leaf replacement. Keep the same
        # three actual module boundaries for both ordinary and native INT8 leaves.
        _no_split_modules = ("Linear", "NativeLinear")

        def __init__(self):
            super().__init__(
                *(
                    torch.nn.Linear(width, width, bias=False, dtype=torch.bfloat16)
                    for _ in factors
                )
            )

    with tempfile.TemporaryDirectory(prefix="cozy-own-va-repeat-") as folder:
        writer = tensorfs.Writer(Path(folder) / "weights.cozytensors")
        from tensorfs import raw

        for index, weight in enumerate(cpu_weights):
            result = raw.quantize(
                "int8.convrot",
                weight.numpy().tobytes(),
                dtype="BF16",
                shape=tuple(weight.shape),
                strategy="int8-convrot256/1",
                params={"rotation": "kron_regular_h4", "rotation_size": 256},
            )
            parts = {
                role: (part.dtype, part.shape, part.tobytes())
                for role, part in result.parts.items()
            }
            writer.component("net").put_encoded(
                f"{index}.weight",
                dtype="BF16",
                shape=tuple(weight.shape),
                scheme=result.scheme,
                params=result.params,
                parts=parts,
            )
        checkpoint = writer.commit()
        chunks = []
        if payload.mode == "sourceerror":
            original_checkpoint = checkpoint
            checkpoint, chunks = private_input(checkpoint, Path(folder))
            original_checkpoint.close()
        root, backend, placement = place(memory, checkpoint, "net", Toy, encoded=True)
        model = backend.models["net"]
        blocks = model.groups()
        assert len(blocks) == 3
        assert all(
            leaf.provider == "cozy.int8-convrot256.native-leaf/1" for leaf in root
        ), [leaf.provider for leaf in root]
        assert all(block.aliases is not None and not block.finals for block in blocks)
        if payload.mode == "mutable_buffer":
            root[0].register_buffer(
                "mutable_counter", torch.zeros((), device=memory.device)
            )
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
            assert weight_bytes > window
            memory.native.set_streaming_limit(window)
            memory.window_bytes = window
            with torch.inference_mode(False):
                data = (
                    torch.arange(width, device=memory.device, dtype=torch.bfloat16)
                    / width
                    + payload.seed % 13
                ).reshape(1, width)
                state = torch.zeros_like(data)
                step = torch.zeros((1,), device=memory.device, dtype=torch.int64)
                output = torch.zeros_like(data)

            def eager_reference(input_data):
                # The actual same native INT8 consumer, served by ordinary Python block scopes.
                from cozy_runtime import graphs

                expected = input_data.clone()
                bypass = graphs.BYPASS.set(True)
                try:
                    with (
                        placement.using("stored-role-reference", ("net",)),
                        torch.no_grad(),
                    ):
                        for outer in range(passes):
                            for leaf in root:
                                expected = leaf(expected)
                            expected.add_(outer + 1)
                    return expected.mul_(3)
                finally:
                    graphs.BYPASS.reset(bypass)

            expected = eager_reference(data)
            changed_data = data + 7
            changed_expected = (
                eager_reference(changed_data) if payload.mode == "reuse" else None
            )
            torch.cuda.synchronize(memory.device)
            assert int8_linear.serving(device_index) == int8_linear.OP, (
                "reference fell back"
            )
            before_policy = memory.native.stats()
            # Ordinary eager reference may retain an entire fitting model and schedule a
            # next-block read. Establish this gate's constrained phase only after it settles.
            with memory.lock:
                memory._settle_prefetch(cancel=True)
            memory._activate_phase(model)
            regions = [memory._native_region(block) for block in blocks]
            pointers = [region.ptr for region in regions]
            for block, region in zip(blocks, regions, strict=True):
                block.preferred = False
                block.preference_set = True
                region.set_preferred(False)
            memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
            memory.window_bytes = window
            memory.native.set_streaming_limit(window)
            admitted_policy = memory.native.stats()
            assert admitted_policy["resident_bytes"] == initial["resident_bytes"]
            assert admitted_policy["pinned_bytes"] == initial["pinned_bytes"]
            tel.log(
                "STORED_ROLE_POLICY "
                + json.dumps(
                    {
                        "before": before_policy["resident_bytes"],
                        "after": admitted_policy["resident_bytes"],
                        "window": memory.window_bytes,
                        "native_window": admitted_policy.get("streaming_limit_bytes"),
                        "groups": len(blocks),
                    }
                )
            )

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
                        escaped.append(root[index].data)
                    if payload.mode == "unselected":
                        state.copy_(root[(index + 1) % len(root)](state))
                        return
                    if payload.mode == "alias_write":
                        root[index].data.view(-1).zero_()
                    if payload.mode == "mutable_buffer":
                        root[index].mutable_counter.add_(1)
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
                    if payload.mode not in {
                        "escaped",
                        "unselected",
                        "alias_write",
                        "mutable_buffer",
                    }:
                        raise
                    from cozy_runtime.author import CapabilityError

                    assert (
                        isinstance(error, CapabilityError)
                        and error.code == "prepared_graph_ineligible"
                    ), error
                    assert index == 0
                    if payload.mode == "escaped":
                        assert escaped
                    if payload.mode == "mutable_buffer":
                        assert root[0].mutable_counter.item() == 0
                    assert_slots(saved)
                    assert not state.count_nonzero().item()
                    return Report(
                        True,
                        json.dumps(
                            {
                                **versions(),
                                "pid": os.getpid(),
                                "mode": payload.mode,
                                "refused": True,
                                "reason": str(error),
                            },
                            sort_keys=True,
                        ),
                    )
                assert int8_linear.serving(device_index) == int8_linear.OP, (
                    "capture fell back"
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
            if payload.mode == "sourceerror":
                memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
                for chunk in chunks:
                    assert (
                        chunk.is_file()
                        and not chunk.is_symlink()
                        and chunk.stat().st_nlink == 1
                    )
                    chunk.write_bytes(b"")
                execution = ctx.plan(program)
                deadline = time.monotonic() + 30
                while not execution.facts()["sequence"].get("done", False):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("source failure did not settle")
                    time.sleep(0.005)
                failure = None
                try:
                    program.close()
                except Exception as error:  # noqa: BLE001 - inspect exact real failure before rethrow
                    failure = error
                assert failure is not None and execution.completion_proven(), failure
                assert program._closed and all(plan._closed for plan in plans)
                assert all(not plan._owned_bindings for plan in plans)
                snapshot = memory.native.stats()
                assert (
                    snapshot["pinned_bytes"] == initial["pinned_bytes"]
                    and not snapshot["poisoned"]
                )
                assert model.users == 1  # fixture's explicit outer claim only
                tel.log(
                    "STORED_ROLE_CLOSE "
                    + json.dumps(
                        {
                            "closed": True,
                            "pins": 0,
                            "users": model.users,
                            "cert": True,
                            "error": type(failure).__name__,
                        }
                    )
                )
                raise failure
            before_calls = dict(calls)
            snapshots = []
            previous = previous_snapshot = None
            submissions = 2 if payload.mode == "reuse" else 1
            for submission in range(submissions):
                if submission:
                    data.copy_(changed_data)
                    expected = changed_expected
                    torch.cuda.synchronize(memory.device)
                    memory.native.reclaim(int(memory.native.stats()["resident_bytes"]))
                execution = ctx.plan(program)
                (actual,) = execution.wait()
                assert int8_linear.serving(device_index) == int8_linear.OP, (
                    "native consumer fell back"
                )
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
                assert after["evictions"] > admitted_policy["evictions"], after
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
                        "actual_int8_kernel": int8_linear.serving(device_index),
                        "native_required": int8_linear.OP,
                        "window_bytes": window,
                        "before_policy": before_policy,
                        "admitted_policy": admitted_policy,
                        "phase_names": sorted(memory._phase_names),
                        "weight_bytes": weight_bytes,
                        "pointers": pointers,
                        "submissions": snapshots,
                        "scope": (
                            "actual native INT8ConvRot stored-role consumer30-pass streaming; "
                            "same-request reuse only; not a Qwen model benchmark"
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
            escaped.clear()
            gc.collect()
            model._release()
            placement.release()
            backend.close()
            memory.trim()
            memory.window_bytes = old_window
            memory.native.set_streaming_limit(old_window)
            cleanup = memory.native.stats()
            assert (
                not cleanup["poisoned"]
                and cleanup["pinned_bytes"] == initial["pinned_bytes"]
            ), cleanup
            checkpoint.close()
