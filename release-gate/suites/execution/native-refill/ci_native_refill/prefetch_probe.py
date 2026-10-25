"""Real next-block admission, demand, cancellation and pressure qualification."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import sys
import tempfile
import time
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.author import Cancelled
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from torch import nn

MIB = 1 << 20


def resident_loop(seed):
    gen = torch.Generator().manual_seed(seed)
    weight = torch.randn(1024, 1024, generator=gen, device="cpu").to("cuda") / 32
    inputs = torch.randn(512, 1024, generator=gen, device="cpu").to("cuda")
    output = torch.empty_like(inputs)
    oracle = torch.relu(inputs @ weight)

    def step(index):
        output.copy_(torch.relu(inputs @ weight))

    return PreparedRepeat.capture(
        step, state=(output,), steps=3000, owners=(weight, inputs)
    ), oracle


def progress(running):
    row = running.poll()
    while not row.done and row.completed_steps == 0:
        time.sleep(0.001)
        row = running.poll()
    assert not row.done
    return row


def issued(read):
    row = read.poll()
    while not row["done"] and not row["issued_bytes"]:
        time.sleep(0.001)
        row = read.poll()
    assert not row["done"] and row["issued_bytes"] > 0
    return row


def automatic(memory, directory, seed, cancel):
    width, large = 1024, 8192
    small = torch.eye(width)
    future = torch.full((large, large), 1 / width)
    writer = tensorfs.Writer(directory / "blocks.cozytensors")
    component = writer.component("net")
    for index, weight in enumerate((small, future, small)):
        component.put(f"{index}.weight", weight)
    checkpoint = writer.commit()
    loop, oracle = resident_loop(seed)
    evidence, tokens = {}, []
    abort = cancel
    observe = True

    class Stage(nn.Module):
        def __init__(self, index):
            super().__init__()
            self.index = index
            shape = (large, large) if index == 1 else (width, width)
            self.weight = nn.Parameter(
                torch.empty(shape, device="meta"), requires_grad=False
            )

        def forward(self, inputs):
            nonlocal observe
            if self.index == 0 and observe:
                pending = memory._prefetch
                assert pending is not None and pending.block.path == "1"
                assert owner.blocks["2"].region is None, "lookahead exceeded one block"
                assert pending.block.data is None and pending.block.assigned is None
                tokens.append(pending.read)
                if abort:
                    evidence["issued_before_cancel"] = issued(pending.read)
                    raise Cancelled("owned cooperative stop before the next block")
                before_read = pending.read.poll()
                assert not before_read["done"], (
                    "fixture did not reach an in-flight prefetch"
                )
                running = loop.submit()
                before = progress(running)
                completed = pending.read.wait()
                after = running.poll()
                assert completed["done"] and not completed["canceled"]
                assert not after.done and after.completed_steps < 3000
                (result,) = running.wait()
                torch.testing.assert_close(result, oracle, rtol=0, atol=0)
                evidence.update(
                    before_read=before_read,
                    producer=completed,
                    compute_steps_before=before.completed_steps,
                    compute_steps_at_publication=after.completed_steps,
                    compute_total_steps=3000,
                )
            return torch.nn.functional.linear(inputs, self.weight[:width, :width])

    module = nn.Sequential(*(Stage(i) for i in range(3)))
    vars(module)["_cozy_weights"] = residency.Declared(
        residency.Weights(checkpoint, "net"), blocks=["0", "1", "2"]
    )
    owner = memory.load(module, "ram")
    inputs = (
        torch.arange(2 * width, dtype=torch.float32).view(2, width).to("cuda") / width
    )
    expected = inputs.cpu() @ small.t() @ future[:width, :width].t() @ small.t()
    try:
        if cancel:
            with memory.request("prefetch-cancel") as stopped, torch.no_grad():
                try:
                    module(inputs)
                except Cancelled:
                    pass
                else:
                    raise AssertionError("owned cancellation did not escape the scope")
            row = tokens[0].poll()
            assert row["done"] and row["canceled"]
            assert (
                memory._prefetch is None and memory.native.stats()["pinned_bytes"] == 0
            )
            evidence.update(after_cancel=row, canceled_request=stopped.stats())

            class Scale:
                identity = "stored-scale-two:1"

                def __call__(self, block, parts):
                    for values in parts.values():
                        for value in values.values():
                            value.mul_(2)

            owner.fill = Scale()
            abort = observe = False
            with memory.request("prefetch-new-fill") as retry, torch.no_grad():
                actual = module(inputs).cpu()
            torch.testing.assert_close(actual, expected * 8, rtol=2e-5, atol=2e-5)
            assert retry.stats()["prefetch"]["started"] == 0
            evidence.update(new_fill=retry.stats(), identity_changed=True)
        else:
            with memory.request("prefetch-cold") as cold, torch.no_grad():
                actual = module(inputs).cpu()
            torch.testing.assert_close(actual, expected, rtol=2e-5, atol=2e-5)
            assert cold.stats()["prefetch"]["started"] >= 1
            assert cold.stats()["prefetch"]["completed"] >= 1
            assert any(row["speculative"] for row in cold.stats()["trace"])
            assert memory._prefetch is None
            observe = False
            with memory.request("prefetch-warm") as warm, torch.no_grad():
                again = module(inputs).cpu()
            torch.testing.assert_close(again, actual, rtol=0, atol=0)
            assert warm.stats()["moved"]["store"]["bytes"] == 0
            assert warm.stats()["prefetch"]["started"] == 0
            evidence.update(
                cold=cold.stats(),
                warm=warm.stats(),
                output_sha256=hashlib.sha256(actual.numpy().tobytes()).hexdigest(),
            )
        assert memory.native.stats()["pinned_bytes"] == 0
    finally:
        loop.close()
        owner.close()
        memory.trim()
        checkpoint.close()
    evidence["after_close"] = memory.native.stats()
    assert evidence["after_close"]["pinned_bytes"] == 0
    return evidence


def pressure(memory, directory, seed):
    from tensorfs import varena

    values = torch.ones(2 * MIB, dtype=torch.uint8)
    writer = tensorfs.Writer(directory / "pressure.cozytensors")
    writer.component("net").put("data", values)
    checkpoint = writer.commit()
    arena = varena.Arena(0)
    arena.set_streaming_limit(2 * MIB)
    resident, future = arena.reserve(2 * MIB), arena.reserve(2 * MIB)
    use = resident.prepare()
    use.tensor.fill_(7)
    use.commit()
    del use
    loop, oracle = resident_loop(seed)
    try:
        running = loop.submit()
        before = progress(running)
        baseline = arena.stats()
        began = time.perf_counter_ns()
        try:
            future.prepare_read(
                checkpoint, [("net/data#value", 0, 2 * MIB)], reclaim=False
            )
        except torch.OutOfMemoryError:
            refusal_ms = (time.perf_counter_ns() - began) / 1e6
        else:
            raise AssertionError("speculation reclaimed the occupied idle window")
        after = running.poll()
        stats = arena.stats()
        assert not after.done
        assert stats["evictions"] == baseline["evictions"]
        assert stats["reclaimed_bytes"] == baseline["reclaimed_bytes"]
        assert resident.resident and not future.resident
        (result,) = running.wait()
        torch.testing.assert_close(result, oracle, rtol=0, atol=0)
        raw = resident.prepare().tensor
        assert torch.all(raw == 7).item()
        del raw
        return {
            "refusal_ms": refusal_ms,
            "before": baseline,
            "after": stats,
            "compute_before": before.completed_steps,
            "compute_after": after.completed_steps,
            "compute_total_steps": 3000,
        }
    finally:
        loop.close()
        arena.reclaim(2 * MIB)
        checkpoint.close()


def exercise(seed, mode):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0", budget_bytes=768 * MIB)
    assert memory.prefetch_supported
    with tempfile.TemporaryDirectory(prefix="cozy-prefetch-") as directory:
        result = (
            pressure(memory, Path(directory), seed)
            if mode == "prefetch-pressure"
            else automatic(memory, Path(directory), seed, mode == "prefetch-cancel")
        )
    return {
        "passed": True,
        "mode": mode,
        "seed": seed,
        "runtime_version": importlib.metadata.version("cozy-runtime"),
        "tensorfs_version": tensorfs.__version__,
        "result": result,
    }


if __name__ == "__main__":
    print(json.dumps({"result": exercise(int(sys.argv[1]), sys.argv[2])}), flush=True)
