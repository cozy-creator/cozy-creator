"""Real installed SDK and CUDA, through the normal Cozy GPU entrypoint."""

import json
from typing import Literal

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 1796
    mode: Literal["owners", "attention"] = "owners"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from .exercise import attention_proof, exercise

    result = (
        attention_proof(payload.seed, ctx)
        if payload.mode == "attention"
        else exercise(payload.seed, ctx)
    )
    return Report(True, json.dumps(result, sort_keys=True))
