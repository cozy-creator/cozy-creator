"""Real installed SDK and CUDA, through the normal Cozy GPU entrypoint."""

import json
from typing import Literal

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 1796
    mode: Literal["owners", "attention", "conv-torch", "conv-varena"] = "owners"


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    if payload.mode in ("conv-torch", "conv-varena"):
        from .convolution import convolution_proof

        result = convolution_proof(payload.seed, ctx, payload.mode.partition("-")[2])
    elif payload.mode == "attention":
        from .attention import attention_proof

        result = attention_proof(payload.seed, ctx)
    else:
        from .exercise import exercise

        result = exercise(payload.seed, ctx)
    return Report(True, json.dumps(result, sort_keys=True))
