"""A GPU call that poisons its own CUDA context, then a healthy successor (executor-fault session).

`fault` prints a marker to its stderr, then trips a device-side assert (an out-of-range index
write), which leaves the context's sticky error: the executor cannot settle quiescent and TensorD
must keep its stderr with the failure. `ok` is the successor: plain arithmetic in a fresh process.
"""

import json
import os
import sys

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    marker: str = ""


class Report(msgspec.Struct):
    pid: int
    detail: str


@app.entrypoint(accelerator=True)
def fault(payload: Probe, ctx: Context) -> Report:
    import torch

    print(f"cuda-fault fixture {payload.marker}: pid {os.getpid()} trips a device-side assert",
          file=sys.stderr, flush=True)
    values = torch.zeros(4, device=str(ctx.device))
    values[torch.tensor([payload.seed % 7 + 4], device=str(ctx.device))] = 1.0
    torch.cuda.synchronize()
    raise RuntimeError("the device-side assert did not fire")


@app.entrypoint(accelerator=True)
def ok(payload: Probe, ctx: Context) -> Report:
    import torch

    value = torch.arange(256, device=str(ctx.device), dtype=torch.int64).sum()
    assert int(value.item()) == 32640
    return Report(os.getpid(), json.dumps({"value": 32640, "seed": payload.seed}))
