"""The sequence. References first, served per step (nothing baked, so no page can carry a stale
overlay): the LoRA (the oracle) and a base call; then, served baked (a new fill identity: every page
goes), the baked LoRA. Then base and LoRA alternate warm with the card free, so most of the other
overlay's pages are resident when each call begins, and again from an emptied cache under a ballast
that leaves the DiT `room_gib`, so blocks are displaced and refilled within every call. Every warm
call must equal its reference: a page's bytes are the same filled now or kept."""

from __future__ import annotations

import json
import time
from functools import partial
from typing import Any

import cozy_runtime
import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author import Context, Telemetry
from cozy_runtime.author._model import PLACED
from cozy_runtime.internal import kernel_ready, lora_bake

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


def _boundary(placement: Any, device: torch.device, kind: str, paging: bool) -> dict[str, Any]:
    """What the device held when an allocation failed: the driver's and Torch's bytes, the weight
    memory's own record, and the scope holds."""
    memory = placement.memory
    facts: dict[str, Any] = {"kind": kind, "paging": paging}
    free, total = torch.cuda.mem_get_info(device)
    facts.update(
        free=free,
        total=total,
        allocated=torch.cuda.memory_allocated(device),
        reserved=torch.cuda.memory_reserved(device),
        peak=torch.cuda.max_memory_allocated(device),
        holds=sorted(getattr(placement, "holds", {})),
        unheld=sorted(getattr(placement, "unheld", ())),
    )
    for name in ("torch_stats", "classes", "room"):
        read = getattr(memory, name, None)
        if callable(read):
            try:
                facts[name] = read()
            except Exception as error:  # a fact that cannot be read says why
                facts[name] = f"unreadable: {error}"
    return facts


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
    # A kernel that lands in the background serves from the next request on (`kernel_ready.ready`
    # never promotes it while one runs); this cell must bake now. The load's background build is
    # left to end, then `wait` takes what it landed, as a request that names its kernel does. A
    # failed build bakes eagerly (`lora_bake.serving`).
    # The bake kernels' slot on this GPU and its opener, as `lora_bake.serving` names them (Runtime #1882: `KERNELS`).
    kernel = f"{lora_bake.KERNELS.name} cuda:{gpu}"
    began = time.monotonic()
    while (slot := kernel_ready._SLOTS.get(kernel)) is not None and slot.state == "opening":
        ctx.raise_if_cancelled()
        waited = int(time.monotonic() - began)
        if waited > 900:
            raise RuntimeError("the LoRA bake kernels did not build in 15 minutes")
        if waited % 30 == 0:
            tel.log("waiting for the LoRA bake kernels", seconds=str(waited))
        time.sleep(1)
    try:
        if not kernel_ready.TRITON:
            raise RuntimeError("no Triton here")
        opener = partial(kernel_ready.TRITON[0], lora_bake.KERNELS, partial(lora_bake.probe, torch, device), device=device)
        kernel_ready.wait(kernel, opener)
    except Exception as error:
        tel.log("the LoRA bake kernels failed; baking eagerly", reason=str(error)[:300])
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

    def call(kind: str, *, empty: bool = False, paging: bool = False) -> dict[str, Any]:
        ctx.raise_if_cancelled()
        torch.cuda.synchronize(device)
        if empty:
            memory.empty()
        ballast = memory.ballast(room) if paging else None
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
        except torch.OutOfMemoryError as error:
            facts = _boundary(placement, device, kind, paging)
            tel.log("lora overlay refill out of memory", facts=json.dumps(facts, default=str))
            raise RuntimeError(
                f"out of memory in a {kind} call: {json.dumps(facts, default=str)}"
            ) from error
        finally:
            del ballast
        row: dict[str, Any] = {
            "kind": kind,
            "empty": empty,
            "serving": bakes.serving,
            "how": bakes.how,
            "baked": placement.baked() - baked,
            "paging": paging,
            "ballast_gib": round(max(memory.capacity - room, 0) / GIB, 2) if paging else 0,
        }
        if trace is not None:
            spans = list(zip(marks, marks[1:], strict=False))
            ours = [
                [e for e in trace[start:stop] if e["set"] == set_name and e["block"] != common]
                for start, stop in spans
            ]
            row["block_fills"] = [len(entries) for entries in ours]
            # A refill from the block's kept baked bytes (Runtime 0.34.3+, origin "baked") bakes
            # nothing: it copies the first bake's bytes back.
            row["store_fills"] = [
                sum(1 for e in entries if e.get("origin", "store") == "store") for entries in ours
            ]
            row["resident_at_start"] = resident
        tel.log("lora overlay refill call", **{k: str(v) for k, v in row.items()})
        return {**row, "out": out}

    # References. Served per step nothing is baked (the hooks add each update), so no page can
    # carry another overlay: the per-step LoRA is the oracle, and the base call's pages are the
    # stored bytes, as a baked base forward's are. Serving baked again changes the fill identity,
    # so every page goes and the baked LoRA fills afresh, with no reclaim that would leave the DiT
    # streaming for the rest of the request.
    placement.serve_lora("per_step")
    oracle = call("lora")
    reference = {"base": call("base")}
    placement.serve_lora("bake")
    reference["lora"] = call("lora")
    lora_effect = _rel(reference["base"]["out"], oracle["out"])
    bake_error = _rel(reference["lora"]["out"], oracle["out"])
    # Base and LoRA alternate twice over: with the card free, so the other overlay's pages are
    # resident when a call begins (the stale page a LoRA forward must not be served), then from an
    # emptied cache under the ballast, so blocks are displaced and refilled within every call too.
    kinds = [kind for _ in range(payload.rounds) for kind in ("base", "lora")]
    warm = [call(kind) for kind in kinds]
    warm += [call(kind, empty=not i, paging=True) for i, kind in enumerate(kinds)]
    failures: list[str] = []
    if reference["lora"]["serving"] != "bake":
        failures.append(f"the LoRA served {reference['lora']['serving']}, not baked")
    # A correct int8 bake rounds W + BA stochastically: 0.12-0.18 (rel. L2) from the per-step LoRA
    # on the 48 GB runs, base 0.25-0.29. The fresh baked reference refills every block, so it
    # must bake each one's LoRA'd linears and land nearer the LoRA than base.
    if reference["lora"]["baked"] < per_block * blocks or not bake_error < lora_effect:
        failures.append(
            f"the baked LoRA baked {reference['lora']['baked']} weights ({per_block * blocks} "
            f"expected) and is {bake_error:.4f} from the per-step LoRA, base {lora_effect:.4f}:"
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
            # What this cell is for: the other overlay's pages resident when a call begins (or it
            # cannot show a stale one), and under the ballast blocks refilled within the call.
            least = 0 if row["empty"] else 1 if row["paging"] else blocks // 2
            if row["resident_at_start"] < least:
                failures.append(
                    f"warm call {index}: {row['resident_at_start']} blocks resident at its start, "
                    f"under {least}: it could not show a stale overlay (run where the DiT fits)"
                )
            if row["paging"] and (len(fills) < 2 or sum(fills[1:]) == 0):
                failures.append(f"warm call {index} did not page within the call: fills {fills}")
            read = sum(row["store_fills"])
            expected = per_block * read if row["kind"] == "lora" else 0
            if row["baked"] != expected:
                failures.append(
                    f"warm call {index} ({row['kind']}) baked {row['baked']} weights for "
                    f"{read} blocks refilled from the store; {expected} expected"
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
