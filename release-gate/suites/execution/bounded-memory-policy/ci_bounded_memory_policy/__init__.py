"""Real installed SDK and CUDA, through the normal Cozy GPU entrypoint."""

import json

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "prefetch"
    seed: int = 1797


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from .exercise import exercise

    return Report(
        True, json.dumps(exercise(payload.mode, payload.seed, ctx), sort_keys=True)
    )
