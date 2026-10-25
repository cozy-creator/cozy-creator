"""Real installed-SDK gate. The candidate function is copied by prepare.py unchanged."""

import json

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 644
    steps: int = 30
    guidance: float = 7.0


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    from .exercise import exercise

    return Report(
        True,
        json.dumps(
            exercise(payload.seed, payload.steps, payload.guidance, ctx, tel),
            sort_keys=True,
        ),
    )
