import json

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "pressure"
    seed: int = 269701


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    from .exercise import exercise

    return Report(
        True, json.dumps(exercise(payload.mode, payload.seed, ctx, tel), sort_keys=True)
    )
