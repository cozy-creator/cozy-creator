"""Installed managed work and its later same-construction successor through Cozy."""

import json
import os

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    pressure: bool = True
    weight_mib: int = 512


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    import torch

    if payload.pressure:
        from .external_managed import exercise

        exercise(ctx, tel, payload.weight_mib)
        raise RuntimeError("the pressure workload was not retired")
    value = torch.arange(256, device=str(ctx.device), dtype=torch.int64).sum()
    assert int(value.item()) == 32640
    return Report(
        True,
        json.dumps({"pid": os.getpid(), "request_id": ctx.request_id, "value": 32640}),
    )
