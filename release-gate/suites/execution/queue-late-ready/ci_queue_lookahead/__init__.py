"""Real deterministic CPU preparation followed by actual native GPU work."""

import hashlib
import json
import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Context, Preflight, Telemetry, UnsupportedInput

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    tag: str
    seed: int
    rounds: Annotated[int, msgspec.Meta(ge=1, le=50_000_000)] = 1
    hold_ms: Annotated[int, msgspec.Meta(ge=0, le=15_000)] = 0
    reject: bool = False


class Facts(msgspec.Struct, frozen=True):
    digest: str


def preflight(payload: Probe) -> Facts:
    # A deterministic, bounded CPU computation with no I/O, device or handler side effects.
    # The long case keeps preparation genuinely in flight when its queued job is canceled.
    if payload.reject:
        raise UnsupportedInput(
            "intentional preflight refusal", code="gate_preflight_refused"
        )
    digest = hashlib.pbkdf2_hmac(
        "sha256",
        payload.tag.encode(),
        str(payload.seed).encode(),
        payload.rounds,
        dklen=16,
    ).hex()
    return Facts(digest)


class Report(msgspec.Struct):
    tag: str
    request_id: str
    pid: int
    entered_ns: int
    completed_ns: int
    digest: str
    value: float


@app.entrypoint(accelerator=True, preflight=preflight)
def gpu(
    payload: Probe, facts: Preflight[Facts], ctx: Context, tel: Telemetry
) -> Report:
    import torch
    from cozy_runtime.compute import PreparedRepeat

    entered = time.time_ns()
    tel.log(
        json.dumps(
            {
                "queue_gate": "entered",
                "tag": payload.tag,
                "request_id": ctx.request_id,
                "pid": os.getpid(),
                "at_ns": entered,
            }
        )
    )
    state = torch.zeros(1, device=str(ctx.device))
    plan = PreparedRepeat.capture(
        lambda index: state.add_(1), state=(state,), steps=30_001
    )
    try:
        execution = ctx.plan(plan)
        # This observer never advances GPU work; the native graph owns the full repeat.
        until = time.monotonic() + payload.hold_ms / 1000
        while time.monotonic() < until:
            ctx.raise_if_cancelled()
            time.sleep(0.05)
        (output,) = execution.wait()
        value = float(output.item())
        assert value == 30_001
        assert plan.facts()["host_graph_launches"] == 1
    finally:
        plan.close()
    return Report(
        payload.tag,
        ctx.request_id,
        os.getpid(),
        entered,
        time.time_ns(),
        facts.digest,
        value,
    )


# Same real workload, distinct entrypoint/construction for the queue-fairness cell.
app.entrypoint(gpu, name="other", accelerator=True, preflight=preflight)


# Real queued GPU preload cells share this existing gate package.
from . import preload  # noqa: E402,F401
