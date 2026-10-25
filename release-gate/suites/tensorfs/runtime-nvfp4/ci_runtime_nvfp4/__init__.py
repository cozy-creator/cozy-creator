"""Actual encoded checkpoints and Runtime providers on the ordinary installed GPU lane."""

import json

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Request, ctx: Context) -> Report:
    from .exercise import exercise

    return Report(True, json.dumps(exercise(payload.seed, ctx), sort_keys=True))
