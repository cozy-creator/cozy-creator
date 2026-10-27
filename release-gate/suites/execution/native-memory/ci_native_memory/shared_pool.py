"""Real ordered scratch pool capture/ownership/output comparison, not a model benchmark."""

from __future__ import annotations

import gc
import json
import os

import msgspec
from cozy_runtime.author import Context


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "shared"
    seed: int = 3289001


class Report(msgspec.Struct):
    passed: bool
    detail: str


def gpu(payload: Probe, ctx: Context) -> Report:
    import torch
    from cozy_runtime.author import CapabilityError
    from cozy_runtime.compute import PreparedRepeat, PreparedSequence

    assert payload.mode in {
        "shared",
        "separate",
        "reuse",
        "escape_view",
        "escape_storage",
        "retire",
    }
    device = torch.device(str(ctx.device))
    with torch.inference_mode(False):
        data = torch.arange(1024, device=device, dtype=torch.float32)
        state = data.clone()
        final = torch.zeros_like(state)
    escaped, calls, stages = [], [0, 0, 0, 0], []
    plan = None
    torch.cuda.empty_cache()
    before = torch.cuda.memory_reserved(device)
    torch.cuda.reset_peak_memory_stats(device)
    recording = None

    def body(index):
        def operation(_step):
            calls[index] += 1
            if payload.mode == "retire":
                recording._retire_if_idle()
            # A64MiB intermediate dies at every stage boundary. Persistent state is outside
            # the pool; subsequent stages consume only that explicit boundary slot.
            temporary = state.expand(16384, -1).clone()
            temporary.add_(index + 1)
            if payload.mode == "escape_view":
                escaped.append(temporary.detach().view(-1))
            elif payload.mode == "escape_storage":
                escaped.append(temporary.untyped_storage())
            state.copy_(temporary[0])

        return operation

    try:
        if payload.mode == "separate":
            for index in range(4):
                stages.append(
                    PreparedRepeat.capture(body(index), state=(state,), steps=1)
                )
            plan = PreparedSequence(stages, outputs=(state,))
        else:
            try:
                with PreparedSequence.record() as recording:
                    for index in range(4):
                        stage = recording.capture(body(index), state=(state,), steps=1)
                        stages.append(stage)
                        try:
                            stage.submit()
                        except RuntimeError as error:
                            assert "preparedsequence" in str(error).casefold()
                        else:
                            raise AssertionError(
                                "shared scratch stage launched independently"
                            )
                    plan = recording.finish(outputs=(state,))
            except (CapabilityError, RuntimeError) as error:
                if payload.mode not in {"escape_view", "escape_storage", "retire"}:
                    raise
                if payload.mode.startswith("escape"):
                    assert (
                        isinstance(error, CapabilityError)
                        and error.code == "prepared_graph_ineligible"
                    )
                    assert "escaped" in str(error).casefold()
                else:
                    assert "during stage preparation" in str(error).casefold()
                assert torch.equal(state, data)
                assert all(stage._closed for stage in stages)
                return Report(
                    True,
                    json.dumps(
                        {
                            "pid": os.getpid(),
                            "mode": payload.mode,
                            "refused": str(error),
                            "state_unchanged": True,
                            "scope": "real capture refusal",
                        }
                    ),
                )
        after = torch.cuda.memory_reserved(device)
        peak = torch.cuda.max_memory_allocated(device)
        initial_calls = list(calls)
        previous = previous_copy = None
        results = []
        for attempt in range(2 if payload.mode == "reuse" else 1):
            state.copy_(data + attempt * 7)
            execution = ctx.plan(plan)
            (actual,) = execution.wait()
            assert torch.equal(actual, data + attempt * 7 + 10)
            assert calls == initial_calls
            if previous is not None:
                assert torch.equal(previous, previous_copy) and not torch.equal(
                    actual, previous
                )
            previous, previous_copy = actual, actual.clone()
            results.append(execution.facts())
        return Report(
            True,
            json.dumps(
                {
                    "pid": os.getpid(),
                    "mode": payload.mode,
                    "reserved_before": before,
                    "reserved_after": after,
                    "peak_allocated": peak,
                    "persistent_boundary_bytes": sum(
                        t.numel() * t.element_size() for t in (data, state, final)
                    ),
                    "facts": results,
                    "calls": calls,
                    "exact_output": True,
                    "scope": (
                        "real shared/separate ordered scratch mechanism; no model performance claim"
                    ),
                },
                sort_keys=True,
            ),
        )
    finally:
        if plan is not None:
            plan.close()
        for stage in stages:
            stage.close()
        escaped.clear()
        gc.collect()
