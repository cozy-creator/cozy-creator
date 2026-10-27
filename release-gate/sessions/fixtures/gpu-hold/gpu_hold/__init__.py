"""An idle executor that keeps GPU memory Runtime does not own (reliability session).

`hold` allocates `mib` MiB as a package global on its first call and keeps it while its process
lives: what a package's own on-GPU cache looks like to TensorD. Offloading its weights (it has
none) frees nothing of it; only ending the process does.
"""

import os

import msgspec
from cozy_runtime.author import App, Context

app = App()
_HELD = []


class Hold(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    mib: int = 2560


class Report(msgspec.Struct):
    pid: int
    held_mib: int


@app.entrypoint(accelerator=True)
def hold(payload: Hold, ctx: Context) -> Report:
    import torch

    if not _HELD:
        _HELD.append(torch.full((payload.mib << 20,), payload.seed % 251, dtype=torch.uint8, device=str(ctx.device)))
    torch.cuda.synchronize()
    return Report(os.getpid(), _HELD[0].numel() >> 20)
