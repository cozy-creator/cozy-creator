"""A plain step loop whose step reads a value back to the host, as a scheduler's `.item()` does (`item`), or
synchronizes the device (`sync`); `warm` only does the arithmetic. Every step runs and the values match a reference
computed beside it. cr-227 (Runtime 0.35.8): a loop capture's refusal of a host read left the capture open, so the
eager rest failed `device_fault`; Runtime 0.43 has no loop capture, and the cell keeps the host reads and syncs."""

import json

import msgspec
import torch
from cozy_runtime import rng
from cozy_runtime.author import App, Context

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
    x = rng.noise((1 << 16,), payload.seed, device=device)
    total = torch.zeros(1, device=device)
    want_x, want_total = x.clone(), total.clone()
    host: list[float] = []

    def step(index: int, x: torch.Tensor, total: torch.Tensor) -> None:
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
    for index in range(STEPS):
        step(index, x, total)
    torch.cuda.synchronize()
    checks = {
        "values": bool(torch.equal(x, want_x) and torch.equal(total, want_total)),
        "host_reads": payload.variant != "item" or len(host) == STEPS,
    }
    detail = json.dumps({"variant": payload.variant, "checks": checks, "request": ctx.request_id}, sort_keys=True)
    if not all(checks.values()):
        raise RuntimeError(f"capture refusal {detail}")
    return Report(True, detail)
