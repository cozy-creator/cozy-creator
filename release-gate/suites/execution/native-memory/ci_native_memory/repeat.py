"""Real finite native ranges with a shared GPU timestep, not the reset stage index."""

from __future__ import annotations

import json
import os
import time

import msgspec
from cozy_runtime.author import Context, Telemetry


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "repeat"
    seed: int = 3268001


class Report(msgspec.Struct):
    passed: bool
    detail: str


def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import torch
    from cozy_runtime.compute import PreparedRepeat, PreparedSequence

    mode = payload.mode
    assert mode in {"repeat", "first-variant", "reuse", "cancel"}
    tel.log(
        "REPEAT_GATE "
        + json.dumps({"pid": os.getpid(), "mode": mode, "seed": payload.seed})
    )
    device = torch.device(str(ctx.device))
    count = 100_000 if mode == "cancel" else 30
    with torch.inference_mode(False):
        parent = torch.arange(256, device=device, dtype=torch.float32)
        original = parent.clone()
        data = parent.clone()
        latent = torch.zeros_like(parent)
        delta = torch.zeros_like(parent)
        first_delta = torch.zeros((2, 256), device=device)
        step = torch.zeros((1,), device=device, dtype=torch.int64)
        output = torch.zeros_like(parent)
    calls = {}

    def capture(name, fn, state, owners=()):
        calls[name] = 0

        def body(index):
            del index
            calls[name] += 1
            fn()

        return PreparedRepeat.capture(body, state=state, steps=1, owners=owners)

    def prefix():
        latent.copy_(data)
        step.zero_()

    def later():
        delta.copy_(latent + step.to(torch.float32).reshape(()))

    def sample():
        latent.copy_(delta + 2)
        step.add_(1)

    def first():
        first_delta[0].copy_(latent + 100)
        first_delta[1].copy_(latent + 200)

    def first_sample():
        latent.copy_(first_delta[0] + 2)
        step.add_(1)

    def suffix():
        output.copy_(latent * 3)

    prefix_plan = capture("prefix", prefix, (latent, step), (data,))
    body_plan = capture("body", later, (delta,), (latent, step))
    sample_plan = capture("sampler", sample, (latent, step), (delta,))
    suffix_plan = capture("suffix", suffix, (output,), (latent,))
    stages = [prefix_plan]
    passes = count
    if mode == "first-variant":
        stages += [
            capture("first-body", first, (first_delta,), (latent,)),
            capture("first-sampler", first_sample, (latent, step), (first_delta,)),
        ]
        passes -= 1
    begin = len(stages)
    stages += [body_plan, sample_plan, suffix_plan]
    plan = PreparedSequence(stages, repeat=(begin, begin + 2, passes))
    try:
        execution = ctx.plan(plan)
        if mode == "cancel":
            deadline = time.monotonic() + 10
            while execution.facts()["sequence"]["completed_passes"] < 2:
                assert time.monotonic() < deadline
                time.sleep(0.001)
            execution.cancel()
            execution.wait()
            facts = execution.facts()
            assert (
                facts["sequence"]["completion_proven"] and facts["sequence"]["canceled"]
            )
            assert 0 < facts["sequence"]["completed_passes"] < count
            assert suffix_plan.facts()["host_graph_launches"] == 0
            assert step.item() == facts["sequence"]["completed_passes"]
        else:
            # These are host-only observations; no Python call advances a stage/pass.
            time.sleep(0.25)
            deadline = time.monotonic() + 10
            while not execution.facts()["sequence"]["done"]:
                assert time.monotonic() < deadline
                time.sleep(0.05)
            (actual,) = execution.wait()
            expected = original.clone()
            for k in range(count):
                expected.add_((100 if mode == "first-variant" and k == 0 else k) + 2)
            expected.mul_(3)
            assert torch.equal(actual, expected), float((actual - expected).abs().max())
            assert step.item() == count
            facts = execution.facts()
            assert facts["sequence"]["completed_passes"] == passes
            assert facts["sequence"]["completed_stages"] == 2 * count + 2
            assert facts["host_graph_launches"] == 2 * count + 2
            if mode == "reuse":
                owner_counts = [x["retained_owner_count"] for x in facts["stages"]]
                saved = actual.clone()
                data.add_(10)
                next_execution = ctx.plan(plan)
                (again,) = next_execution.wait()
                assert torch.equal(again, expected + 30)
                assert torch.equal(actual, saved)
                facts = next_execution.facts()
                assert facts["sequence"]["completed_passes"] == count
                assert facts["host_graph_launches"] == 2 * (2 * count + 2)
                assert [
                    x["retained_owner_count"] for x in facts["stages"]
                ] == owner_counts
                assert step.item() == count
        assert all(value == 2 for value in calls.values()), calls
        assert torch.equal(parent, original)
        return Report(
            True,
            json.dumps(
                {
                    "pid": os.getpid(),
                    "mode": mode,
                    "native": facts,
                    "gpu_outer_step": step.item(),
                    "python_calls": calls,
                    "parent_unchanged": True,
                    "scope": "real resident30-pass schedule and explicit first/later shapes; no full-model or streamed-weight claim",
                },
                sort_keys=True,
            ),
        )
    finally:
        plan.close()
