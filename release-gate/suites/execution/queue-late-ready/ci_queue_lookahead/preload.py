"""Actual same-construction queued preload and later model consumption."""

from __future__ import annotations

import hashlib
import json
import os
import time
from typing import Any, Literal

import msgspec
import torch

from cozy_runtime.author import (
    Context,
    Loader,
    Model,
    ModelArtifact,
    Preflight,
    Telemetry,
    WeightsOutput,
    uses_components,
)
from cozy_runtime.compute import PreparedRepeat, PreparedSequence
from cozy_runtime.internal.residency import _REQUEST
from cozy_runtime.weights_store import Derivation

from . import app

WIDTH = 4096


class Pipeline:
    def __init__(self, config: Any) -> None:
        self.width = config.as_int("width")
        self.gain = config.as_int("gain")
        self.components = {
            "next": torch.nn.Linear(self.width, self.width, bias=False, dtype=torch.float32)
        }
        self.preloaded_region = None


def build(config: Any) -> Pipeline:
    return Pipeline(config)


class PreloadModel(Model[Pipeline], encoded_leaves="refuse"):
    pipe: Pipeline

    def load(self, loader: Loader) -> None:
        self.pipe = loader.construct(Pipeline, factory=build)

    @uses_components("next")
    def consume(self, value: float) -> torch.Tensor:
        layer = self.pipe.components["next"]
        inputs = torch.full((1, self.pipe.width), value, device=layer.weight.device)
        return layer(inputs)


class Source(msgspec.Struct, forbid_unknown_fields=True):
    seed: int


class PreloadInput(msgspec.Struct, forbid_unknown_fields=True):
    tag: Literal["A", "B", "C"]
    seed: int
    mode: Literal["overlap", "capacity", "cancel", "control"] = "overlap"
    rounds: int = 1
    observer_park_ms: int = 250
    program: Literal["repeat", "sequence"] = "repeat"
    steps: int = 1_000_001


class PreloadReport(msgspec.Struct):
    tag: str
    request_id: str
    pid: int
    passed: bool
    detail: str


@app.entrypoint(accelerator=False, weights=(WeightsOutput("model", 65 << 20),))
def preload_source(payload: Source, ctx: Context) -> ModelArtifact:
    gain = 1 + payload.seed % 3
    weight = torch.full((WIDTH, WIDTH), gain / WIDTH, dtype=torch.float32)
    with ctx.output("model", Derivation(base=None, order=(("next", "weight"),))) as output:
        output.config("next", {"width": WIDTH, "gain": gain, "seed": payload.seed})
        output.put("next", "weight", "F32", weight.shape, weight.numpy().tobytes())
        return ctx.adopt_model(output)


class PreloadFacts(msgspec.Struct, frozen=True):
    digest: str


def prepare_preload(payload: PreloadInput) -> PreloadFacts:
    assert 1 <= payload.rounds <= 50_000_000
    # Genuine bounded CPU work; it neither waits for nor changes the running GPU program.
    digest = hashlib.pbkdf2_hmac(
        "sha256", payload.tag.encode(), str(payload.seed).encode(), payload.rounds, dklen=16
    ).hex()
    return PreloadFacts(digest)


@app.entrypoint(accelerator=True, phases=("next",), preflight=prepare_preload)
def preload_gpu(
    payload: PreloadInput,
    facts: Preflight[PreloadFacts],
    model: PreloadModel,
    ctx: Context,
    tel: Telemetry,
) -> PreloadReport:
    assert ctx.device.type == "cuda"
    request = _REQUEST.get()
    assert request is not None and not request.overlap
    memory = request.memory
    root = model.pipe.components["next"]
    owned = memory.models[root]
    (block,) = tuple(owned.groups())
    assert block.storage_bytes == WIDTH * WIDTH * 4
    assert block.aliases is not None and owned.fill is None and not block.finals
    evidence: dict[str, Any] = {
        "tag": payload.tag,
        "mode": payload.mode,
        "checkpoint": owned.source.ck.id,
        "component": owned.component,
        "stored_bytes": block.storage_bytes,
        "preflight_digest": facts.digest,
    }
    if payload.tag in {"B", "C"}:
        tel.log(
            json.dumps(
                {
                    "queue_gate": "entered",
                    "tag": payload.tag,
                    "request_id": ctx.request_id,
                    "pid": os.getpid(),
                    "at_ns": time.time_ns(),
                }
            )
        )
        before = request.moved
        cached = block.cached()
        same_region = block.region is model.pipe.preloaded_region
        value = float(1 + payload.seed % 7)
        output = model.consume(value)
        torch.testing.assert_close(
            output, torch.full_like(output, value * model.pipe.gain), rtol=0, atol=0
        )
        moved = request.moved - before
        if payload.mode == "overlap":
            assert cached and same_region, (cached, same_region)
            assert moved == 0, request.stats()
        else:
            assert payload.mode == "capacity" and moved == block.storage_bytes, request.stats()
        evidence.update(
            cached=cached, same_region=same_region, demand_bytes=moved, value=float(output[0, 0])
        )
        return PreloadReport(payload.tag, ctx.request_id, os.getpid(), True, json.dumps(evidence))

    assert not block.cached(), "Use a new source commit/seed; cached input is not a preload proof"
    assert 30001 <= payload.steps <= 2_000_000
    with torch.inference_mode(False), torch.no_grad():
        state = torch.zeros(1, device=str(ctx.device))
    calls = [0]

    def step(index: torch.Tensor) -> None:
        del index
        calls[0] += 1
        state.add_(1)

    plan = PreparedRepeat.capture(step, state=(state,), steps=payload.steps)
    launches = 1
    native_steps = payload.steps
    if payload.program == "sequence":
        reset = PreparedRepeat.capture(lambda _: state.zero_(), state=(state,), steps=1)
        plan = PreparedSequence((reset, plan), outputs=(state,))
        launches, native_steps = 2, payload.steps + 1
    evidence["program"] = payload.program
    ballast = None
    try:
        if payload.mode == "capacity":
            # Keep a real live allocation: make spare room smaller than one target block
            # plus the existing current-phase window, while keeping CUDA's reserve intact.
            remaining = block.allocation_bytes // 2
            take = max(memory.room().free - remaining, 0)
            assert take > 0
            ballast = torch.empty(take, dtype=torch.uint8, device=str(ctx.device))
            assert memory.room().free < block.allocation_bytes
        assert request.queued is not None and request.queued() is None, (
            "controller must submit B only after A's native program has started"
        )
        evictions_before = int(memory.native.stats()["evictions"])
        started = torch.cuda.Event(enable_timing=True)
        started.record(torch.cuda.current_stream())
        submitted_ns = time.time_ns()
        execution = ctx.plan(plan)
        done = torch.cuda.Event(enable_timing=True)
        done.record(torch.cuda.current_stream())
        tel.log(
            json.dumps(
                {
                    "queue_gate": "entered",
                    "tag": payload.tag,
                    "request_id": ctx.request_id,
                    "pid": os.getpid(),
                    "at_ns": time.time_ns(),
                    "native_submitted_ns": submitted_ns,
                }
            )
        )
        assert 0 <= payload.observer_park_ms <= 1000

        def poll():
            return (
                plan._native.poll()
                if payload.program == "sequence"
                else plan._native.poll(execution._epoch)
            )

        def native_progress():
            # A native poll answers with the sample its previous poll requested (a sequence
            # stage's first poll has none yet): request one now, then read it.
            poll()
            time.sleep(0.01)
            return int(poll()[0])

        def native_done():
            # The program's own completion: a sequence's worker runs its stages on their own
            # streams, so an event recorded on this stream after submission proves nothing.
            return bool(poll()[2])

        before_park = native_progress()
        if payload.observer_park_ms:
            time.sleep(payload.observer_park_ms / 1000)
        after_park = native_progress()
        assert after_park > before_park, "native GPU program needed its Python observer"
        evidence.update(
            native_submitted_ns=submitted_ns,
            progress_before_park=before_park,
            progress_after_park=after_park,
            observer_park_ms=payload.observer_park_ms,
        )
        if payload.mode == "control":
            (actual,) = execution.wait()
            assert float(actual.item()) == payload.steps
            assert plan.facts()["host_graph_launches"] == launches
            assert not request.queued_prefetch
            evidence.update(
                native=plan.facts(),
                current_result=float(actual.item()),
                current_gpu_ms=started.elapsed_time(done),
                control=True,
            )
            return PreloadReport(
                payload.tag, ctx.request_id, os.getpid(), True, json.dumps(evidence)
            )
        deadline = time.monotonic() + 90
        offered = None
        while offered is None:
            ctx.raise_if_cancelled()
            assert time.monotonic() < deadline
            assert not native_done(), "B's late CPU preflight missed A's active native window"
            progress = execution.poll()  # only the production observer may offer the read
            assert not progress.done, "A completed before B became CPU ready"
            offered = request.queued()
            if offered is None:
                time.sleep(0.01)
        ready_observed_ns = time.time_ns()
        assert ready_observed_ns > submitted_ns
        evidence.update(queued_request=offered.request_id, cpu_ready_observed_ns=ready_observed_ns)
        # poll again if the prepared record appeared immediately after the previous poll.
        execution.poll()
        pending = memory._prefetch
        if payload.mode == "capacity":
            assert pending is None
            assert any(row["state"] == "capacity_skip" for row in request.queued_prefetch)
            for _ in range(10):
                execution.poll()
            skips = [row for row in request.queued_prefetch if row["state"] == "capacity_skip"]
            assert len(skips) == 1, skips  # one offer, including refusal, per native epoch
            evidence["capacity_skip"] = list(request.queued_prefetch)
        else:
            assert pending is not None and pending.queued is not None
            assert pending.queued.request_id == offered.request_id
            model.pipe.preloaded_region = pending.region
            tel.log(
                json.dumps(
                    {
                        "queue_gpu_preload": "started",
                        "request_id": ctx.request_id,
                        "queued_request": offered.request_id,
                        "at_ns": time.time_ns(),
                    }
                )
            )
            # This observer never starts or advances the read. Its native worker must finish
            # actual copied bytes while the independently running GPU program is incomplete.
            while True:
                ctx.raise_if_cancelled()
                read = dict(pending.read.poll())
                if read["done"]:
                    break
                assert not native_done(), "GPU program finished before native input read"
                time.sleep(0.001)
            gpu_done = native_done()
            if payload.mode == "overlap":
                assert not gpu_done and not read["canceled"], (gpu_done, read)
                assert read["bytes"] == block.storage_bytes, read
                assert read["direct_bytes"] + read["staged_bytes"] > 0, read
            evidence.update(native_read=read, gpu_complete_at_read_done=gpu_done)
            if payload.mode == "cancel":
                while offered.valid():
                    ctx.raise_if_cancelled()
                    assert not native_done(), "B cancel missed the active-program window"
                    time.sleep(0.005)
                evidence["queued_cancel_observed"] = True
                evidence["cancel_outcome"] = (
                    "canceled_read" if read["canceled"] else "fill_completed_before_cancel"
                )
        (actual,) = execution.wait()
        progress = execution.poll()
        assert not progress.canceled and progress.completed_steps == native_steps
        assert float(actual.item()) == payload.steps
        assert calls[0] == 2 and plan.facts()["host_graph_launches"] == launches
        assert int(memory.native.stats()["evictions"]) == evictions_before
        evidence.update(
            native=plan.facts(),
            preparation_calls=calls[0],
            current_result=float(actual.item()),
            current_gpu_ms=started.elapsed_time(done),
            evictions_before=evictions_before,
            evictions_after=int(memory.native.stats()["evictions"]),
        )
    finally:
        plan.close()
        del ballast
    return PreloadReport(payload.tag, ctx.request_id, os.getpid(), True, json.dumps(evidence))
