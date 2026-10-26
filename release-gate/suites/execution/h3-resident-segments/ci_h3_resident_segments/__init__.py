"""Real CUDA tinyH3 component; fullmodel requests remain15seconds/30steps."""

import json

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 2697


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from .exercise import exercise

    return Report(True, json.dumps(exercise(payload.seed, ctx), sort_keys=True))
