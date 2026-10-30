"""A `denoise_loop` whose step reads a value back to the host, as a scheduler's `.item()` does (`item`), or
synchronizes the device (`sync`); `warm` only does the arithmetic, so the executor's kernels are ready (a new
executor's first loop is refused before its capture while they open). No capture records a host read: the recording fails with a CUDA capture error, the loop
refuses it and runs the rest eagerly, and the values match an eager reference. cr-227 (Runtime 0.35.8): the failed
capture stayed open after its refusal, so the eager rest failed `device_fault`."""

import json

import msgspec
import torch
from cozy_runtime.author import App, Context, denoise_loop
from cozy_runtime.internal import loop

app = App()
STEPS = 4


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    variant: str = "item"
    seed: int = 351


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    if payload.variant not in ("warm", "item", "sync"):
        raise ValueError(f"unknown variant {payload.variant}; warm, item or sync")
    device = torch.device("cuda", torch.cuda.current_device())
    x = torch.randn(1 << 16, device=device, generator=torch.Generator(device).manual_seed(payload.seed))
    total = torch.zeros(1, device=device)
    want_x, want_total = x.clone(), total.clone()
    host: list[float] = []

    def step(index, x, total):
        del index
        x.mul_(0.5)
        total.add_(x.sum())
        if payload.variant == "item":
            host.append(total.item())
        elif payload.variant == "sync":
            torch.cuda.synchronize()

    for _ in range(STEPS):
        want_x.mul_(0.5)
        want_total.add_(want_x.sum())
    denoise_loop(step, range(STEPS), state=(x, total))
    torch.cuda.synchronize()
    row = (loop.ROWS.get(ctx.request_id) or [{}])[-1]
    checks = {
        "values": bool(torch.equal(x, want_x) and torch.equal(total, want_total)),
        "host_reads": payload.variant != "item" or len(host) == STEPS,
        "refused": payload.variant == "warm"
        or (row.get("mode") == "eager" and any("cannot be recorded" in r for r in row.get("refusals") or ())),
    }
    detail = json.dumps({"variant": payload.variant, "checks": checks,
                         "loop": {k: row.get(k) for k in ("mode", "native_steps", "refusals")}}, sort_keys=True)
    if not all(checks.values()):
        raise RuntimeError(f"capture refusal {detail}")
    return Report(True, detail)
