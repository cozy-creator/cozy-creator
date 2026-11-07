# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.39", "msgspec>=0.19", "torch>=2.14"]
# ///
"""A function with no model holds its GPUs in one process: a block on each, worked on, and held while they're read."""
import json
import os
import time

import msgspec
import torch

from cozy_runtime.author import App

app = App()


class Hold(msgspec.Struct):
    gib: float = 1.0
    seconds: float = 45.0


class Held(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def hold(payload: Hold) -> Held:
    gpus = range(torch.cuda.device_count())
    free = [torch.cuda.mem_get_info(gpu)[0] for gpu in gpus]
    blocks = [torch.ones(int(payload.gib * (1 << 30)) // 4, device=f"cuda:{gpu}") for gpu in gpus]
    means = [float((block * 2).mean()) for block in blocks]
    allocated = [torch.cuda.memory_allocated(gpu) for gpu in gpus]
    taken = [before - torch.cuda.mem_get_info(gpu)[0] for gpu, before in zip(gpus, free)]
    time.sleep(payload.seconds)
    want = int(payload.gib * (1 << 30))
    # What it holds, not what the driver lost: a process kept from before serves it from its own idle pages.
    passed = len(gpus) >= 2 and all(mean == 2 for mean in means) and all(bytes >= want for bytes in allocated)
    return Held(passed, json.dumps({"pid": os.getpid(), "means": means, "allocated": allocated, "taken": taken}))
