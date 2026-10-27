"""The sequence. References first, each from an emptied cache with the whole card free (the DiT
fits, so a scope that binds it before its forward arms fills every block unarmed): a per-step LoRA
(no bake, so no page can carry a stale overlay), then a baked LoRA and a base call. Then warm calls
alternate base and LoRA under a ballast that leaves the DiT `room_gib`: blocks page within every
call and the resident ones carry over into the next call, whose overlay differs. Every warm call
must equal its reference: a page's bytes are the same filled now or kept."""

from __future__ import annotations

import time
from typing import Any

import cozy_runtime
import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author import Context, Telemetry
from cozy_runtime.author._model import PLACED
from cozy_runtime.internal import lora_bake

from . import Inputs, Lightning, Probe, QwenDiT

GIB = 1 << 30
#: A warm call equals its reference within this (rel. L2 on the latents); a stale page is far more.
SAME = 1e-3


def _rel(a: torch.Tensor, b: torch.Tensor) -> float:
    return float((a - b).norm() / b.norm())


class _Memory:
    """The device's weight memory as this Runtime exposes it: 0.34+ Runtime residency (explicit
    reclaim before a ballast, so no allocation leans on the allocator hook), else 0.33's varena,
    which is Torch's allocator and evicts idle pages for an allocation."""

    def __init__(self, placement: Any, device: torch.device) -> None:
        self.memory, self.device = placement.memory, device
        self.native = callable(getattr(self.memory, "reclaim", None))
        self.capacity = 0

    def empty(self) -> None:
        """Every idle page dropped: the next call fills each block afresh."""
        if self.native:
            self.memory.reclaim(1 << 62)
            self.capacity = max(self.capacity, int(self.memory.room().free))
            return
        held = self._largest(torch.cuda.get_device_properties(self.device).total_memory)
        self.capacity = max(self.capacity, held.numel())
        del held

    def ballast(self, room: int) -> torch.Tensor:
        """Bytes held through a call so only `room` is left for weights."""
        want = max(self.capacity - room, 0)
        if self.native:
            self.memory.reclaim(want)
            return torch.empty(want, dtype=torch.uint8, device=self.device)
        return self._largest(want)

    def _largest(self, limit: int) -> torch.Tensor:
        size = limit
        while size > 0:
            try:
                return torch.empty(size, dtype=torch.uint8, device=self.device)
            except torch.OutOfMemoryError:
                size -= 256 << 20
        return torch.empty(0, dtype=torch.uint8, device=self.device)


def _request_trace() -> list[dict[str, Any]] | None:
    """This request's fill trace (Runtime 0.34+), else None."""
    try:
        from cozy_runtime.internal import residency
    except ImportError:
        return None
    request = residency._REQUEST.get()
    return None if request is None else request.trace


def exercise(
    payload: Probe, model: QwenDiT, turbo_lora: Lightning, ctx: Context, tel: Telemetry
) -> dict[str, Any]:
    dit = model.graph.components["transformer"]
    placement, name = PLACED[dit]
    bakes = placement.bakes[name]
    device = torch.device(str(model.device))
    gpu = device.index or 0
    began = time.monotonic()
    while lora_bake.serving(gpu) is None:  # a bake waits for no kernel; this cell must bake
        ctx.raise_if_cancelled()
        waited = int(time.monotonic() - began)
        if waited > 900:
            raise RuntimeError("the LoRA bake kernels did not build in 15 minutes")
        if waited % 30 == 0:
            tel.log("waiting for the LoRA bake kernels", seconds=str(waited))
        time.sleep(1)
    held = placement.models[name]
    set_name, blocks = held.name, len(held.blocks)
    # Weights a refilled block bakes: its hooked linears this lane bakes into the page (an svdq
    # leaf adopts its LoRAs as it runs instead, and pages nothing; this cell needs a baking lane).
    per_block = sum(1 for b in bakes.bakes.values() if b.hooks and b.leaf is None) // blocks
    if not per_block:
        raise RuntimeError("this lane bakes no LoRA into pages; run the cell on an int8/fp8 lane")
    common = getattr(held.common, "index", None) if _request_trace() is not None else None
    memory = _Memory(placement, device)
    room = int(payload.room_gib * GIB)
    inputs = Inputs(payload.seed, payload.size, payload.steps)

    def call(kind: str, *, empty: bool) -> dict[str, Any]:
        ctx.raise_if_cancelled()
        torch.cuda.synchronize(device)
        if empty:
            memory.empty()
        ballast = None if empty else memory.ballast(room)
        trace = _request_trace()
        resident = (
            sum(1 for b in held.blocks.values() if b.region is not None and b.region.resident)
            if trace is not None
            else None
        )
        marks: list[int] = [len(trace) if trace is not None else 0]
        baked = placement.baked()

        def after(_: int) -> None:
            marks.append(len(trace) if trace is not None else 0)

        try:
            if kind == "base":
                out = model.denoise(inputs, None, after)
            else:
                out = turbo_lora.denoise(model, inputs, after)
        finally:
            del ballast
        row: dict[str, Any] = {
            "kind": kind,
            "empty": empty,
            "serving": bakes.serving,
            "how": bakes.how,
            "baked": placement.baked() - baked,
            "ballast_gib": 0 if empty else round(max(memory.capacity - room, 0) / GIB, 2),
        }
        if trace is not None:
            fills = [
                sum(
                    1
                    for entry in trace[start:stop]
                    if entry["set"] == set_name and entry["block"] != common
                )
                for start, stop in zip(marks, marks[1:], strict=False)
            ]
            row["block_fills"], row["resident_at_start"] = fills, resident
        tel.log("lora overlay refill call", **{k: str(v) for k, v in row.items()})
        return {**row, "out": out}

    # References, each from an emptied cache. The per-step LoRA bakes nothing (its hooks add each
    # update), so no page can carry another overlay: it is the oracle the baked LoRA must match.
    placement.serve_lora("per_step")
    oracle = call("lora", empty=True)
    placement.serve_lora("bake")
    reference = {"lora": call("lora", empty=True), "base": call("base", empty=True)}
    lora_effect = _rel(reference["base"]["out"], oracle["out"])
    bake_error = _rel(reference["lora"]["out"], oracle["out"])
    # Base and LoRA alternate; the last LoRA follows a LoRA, whose resident pages it must reuse.
    kinds = [kind for _ in range(payload.rounds) for kind in ("base", "lora")] + ["lora"]
    warm = [call(kind, empty=False) for kind in kinds]
    failures: list[str] = []
    if reference["lora"]["serving"] != "bake":
        failures.append(f"the LoRA served {reference['lora']['serving']}, not baked")
    if not bake_error < 0.5 * lora_effect:
        failures.append(
            f"the baked LoRA is {bake_error:.4f} from the per-step LoRA and base {lora_effect:.4f}:"
            " the bake did not apply its factors"
        )
    rows = []
    for index, row in enumerate(warm):
        error = _rel(row["out"], reference[row["kind"]]["out"])
        row.update(
            error=round(error, 6), exact=torch.equal(row["out"], reference[row["kind"]]["out"])
        )
        if error > SAME:
            failures.append(f"warm call {index} ({row['kind']}) is {error:.4f} from its reference")
        fills = row.get("block_fills")
        if fills is not None:
            # The paging this cell is for: pages of the previous call were resident when it began,
            # and blocks were displaced and refilled within it. After a call of the same overlay
            # its resident pages serve again: the first forward refills fewer than every block.
            if not row["resident_at_start"] or len(fills) < 2 or sum(fills[1:]) == 0:
                failures.append(
                    f"warm call {index} did not page as intended: {row['resident_at_start']} "
                    f"blocks resident at its start, fills {fills} per forward"
                )
            if index and warm[index - 1]["kind"] == row["kind"] and fills[0] >= blocks:
                failures.append(f"warm call {index} refilled every block after the same overlay")
            expected = per_block * sum(fills) if row["kind"] == "lora" else 0
            if row["baked"] != expected:
                failures.append(
                    f"warm call {index} ({row['kind']}) baked {row['baked']} weights for "
                    f"{sum(fills)} refilled blocks; {expected} expected"
                )
        rows.append({k: v for k, v in row.items() if k != "out"})
    return {
        "passed": not failures,
        "runtime": cozy_runtime.__version__,
        "runtime_commit": _build_provenance.COMMIT,
        "failures": failures,
        "lora_effect": round(lora_effect, 6),
        "bake_error": round(bake_error, 6),
        "references": {
            key: {k: v for k, v in row.items() if k != "out"}
            for key, row in {"per_step": oracle, **reference}.items()
        },
        "warm": rows,
        "room_gib": payload.room_gib,
        "capacity_gib": round(memory.capacity / GIB, 2),
        "trace": _request_trace() is not None,
        "baked_per_block": per_block,
    }
