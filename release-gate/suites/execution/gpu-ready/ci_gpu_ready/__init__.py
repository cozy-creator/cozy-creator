"""A weightless GPU request: whether ready work is admitted while another request's inputs fetch."""

import os
import time

import msgspec
import torch
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    tag: str = "ready"


class Result(msgspec.Struct):
    passed: bool
    pid: int
    tag: str
    entered_ms: int


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Result:
    entered = time.time_ns() // 1_000_000
    a = torch.arange(1024, device=str(ctx.device), dtype=torch.float32)
    assert float((a + 1).sum()) == 524800
    return Result(True, os.getpid(), payload.tag, entered)
