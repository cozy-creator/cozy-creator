"""Installed SDK proof through the ordinary Cozy GPU lane; no private interpreter."""

import json

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "loaded"
    seed: int = 103
    dtype: str = "F32"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    if payload.mode.startswith("phase-"):
        from .phases import exercise

        result = exercise(payload.mode, payload.seed, ctx)
    elif payload.mode.startswith("encoded-"):
        from .encoded import exercise

        result = exercise(payload.mode, payload.seed, payload.dtype, ctx)
    else:
        from .exercise import exercise

        result = exercise(payload.mode, payload.seed, ctx, tel)
    return Report(True, json.dumps(result, sort_keys=True))
