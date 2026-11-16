"""GPU calls for the hang session (cl-977 G9): one that deadlocks inside its steps while the Runtime's step sampler ticks, and
one whose step is a single long kernel."""

import threading
import time

import msgspec
from cozy_runtime.author import App, Context, Telemetry

app = App()


class Steps(msgspec.Struct, forbid_unknown_fields=True):
    seed: int
    seconds: float = 90.0


class Done(msgspec.Struct):
    value: float


@app.entrypoint(accelerator=True)
def stuck(payload: Steps, ctx: Context, tel: Telemetry) -> Done:
    """Two real steps on its GPU, then a lock it holds, inside the third: its GPU idle, the step sampler ticking."""
    import torch

    x = torch.full((4096, 4096), 1e-3 * (payload.seed % 7 + 1), device=str(ctx.device))
    with tel.stage("denoise"):
        for step in range(4):
            x = (x @ x).tanh()
            torch.cuda.synchronize()
            ctx.progress(step, 4)
            if step == 1:
                held = threading.Lock()
                held.acquire()
                held.acquire()
    return Done(float(x.mean()))


@app.entrypoint(accelerator=True)
def long(payload: Steps, ctx: Context, tel: Telemetry) -> Done:
    """A step, then one that is a single GPU kernel for `seconds` (`torch.cuda._sleep`), with no word between."""
    import torch

    with tel.stage("denoise"):
        torch.cuda.synchronize()
        began = time.perf_counter()
        torch.cuda._sleep(100_000_000)
        torch.cuda.synchronize()
        rate = 100_000_000 / (time.perf_counter() - began)
        ctx.progress(0, 2)
        began = time.perf_counter()
        torch.cuda._sleep(int(rate * payload.seconds))
        torch.cuda.synchronize()
        ctx.progress(1, 2)
    return Done(time.perf_counter() - began)
