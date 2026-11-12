# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.39", "msgspec>=0.19", "torch>=2.14"]
# ///
"""Holds most of each GPU's memory and pinned host memory until it is canceled: its kill leaves the driver all of it to release."""
import os
import time

import msgspec
import torch

from cozy_runtime.author import App

app = App()


class Hold(msgspec.Struct):
    gib: float = 12.0
    host_gib: float = 16.0
    seconds: float = 900.0


class Held(msgspec.Struct):
    pid: int


@app.entrypoint(accelerator=True)
def hold(payload: Hold) -> Held:
    blocks = [torch.ones(int(payload.gib * (1 << 30)) // 4, device=f"cuda:{gpu}") for gpu in range(torch.cuda.device_count())]
    pinned = torch.empty(int(payload.host_gib * (1 << 30)), dtype=torch.uint8, pin_memory=True).fill_(1)
    print(f"holding: pid {os.getpid()}, {len(blocks)} GPUs, {pinned.numel() >> 30} GiB pinned", flush=True)
    time.sleep(payload.seconds)
    return Held(os.getpid())
