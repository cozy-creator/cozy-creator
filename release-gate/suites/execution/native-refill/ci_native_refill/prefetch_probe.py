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


def capture_exclusion(memory, directory):
    from tensorfs import varena

    values = torch.ones(256 * MIB, dtype=torch.uint8)
    writer = tensorfs.Writer(directory / "capture.cozytensors")
    writer.component("net").put("data", values)
    checkpoint = writer.commit()
    parts = [("net/data#value", 0, values.nbytes)]
    region = memory.native.reserve(values.nbytes)
    unstarted = region.prepare_read(checkpoint, parts, reclaim=False)

    def refused():
        try:
            with varena.capture(0):
                pass
        except RuntimeError as error:
            assert "producer" in str(error)
            return str(error)
        raise AssertionError("actual capture admitted an outstanding producer")

    before_start = refused()
    unstarted.cancel()
    assert unstarted.wait()["canceled"]
    with varena.capture(0):
        pass
    memory.native.reclaim(region.allocated_bytes)
    active = region.prepare_read(checkpoint, parts, threads=1, reclaim=False)
    active.start()
    in_flight = issued(active)
    during_read = refused()
    active.cancel()
    ended = active.wait()
    assert ended["canceled"]
    memory.native.reclaim(region.allocated_bytes)

    stream = torch.cuda.Stream()
    graph = torch.cuda.CUDAGraph()
    inputs = torch.ones(32, device="cuda")
    output = torch.empty_like(inputs)
    with torch.cuda.stream(stream):
        output.copy_(inputs)
    stream.synchronize()
    try:
        with varena.capture(0), torch.cuda.graph(graph, stream=stream):
            try:
                region.prepare_read(checkpoint, parts, reclaim=False)
            except RuntimeError as error:
                assert "capture" in str(error)
                during_capture = str(error)
            else:
                raise AssertionError(
                    "producer admission succeeded inside actual CUDA capture"
                )
            try:
                with varena.replay(0):
                    pass
            except RuntimeError as error:
                replay_in_capture = str(error)
            else:
                raise AssertionError("replay guard entered actual capture")
            output.copy_(inputs)
        with varena.replay(0):
            try:
                with varena.capture(0):
                    pass
            except RuntimeError as error:
                capture_in_replay = str(error)
            else:
                raise AssertionError("actual capture entered a replay guard")
            try:
                memory.native.reclaim(1)
            except RuntimeError as error:
                reclaim_in_replay = str(error)
            else:
                raise AssertionError("replay guard permitted native reclamation")
            graph.replay()
        torch.testing.assert_close(output, inputs, rtol=0, atol=0)
        assert not region.resident and memory.native.stats()["pinned_bytes"] == 0
        return {
            "before_start_refusal": before_start,
            "issued": in_flight,
            "active_refusal": during_read,
            "ended": ended,
            "during_actual_capture_refusal": during_capture,
            "graph_replay_passed": True,
            "replay_in_capture_refusal": replay_in_capture,
            "capture_in_replay_refusal": capture_in_replay,
            "reclaim_in_replay_refusal": reclaim_in_replay,
        }
    finally:
        graph.reset()
        checkpoint.close()


def ordinary_replay(memory, directory):
    from cozy_runtime import graphs

    class Elementwise(nn.Module):
        def forward(self, values):
            for _ in range(64):
                values = torch.relu(values * 0.99 + 0.001)
            return values

    module = Elementwise().cuda().eval()
    inputs = torch.ones(16, 128, device="cuda")
    graphs.install(torch, object(), {"net": module})
    with torch.no_grad():
        for _ in range(3):
            expected = module(inputs)
        torch.cuda.synchronize()
        graphs.capture_idle(lambda: False)
        baseline = graphs.describe(module)
        assert baseline["active_graphs"] > 0, baseline
        actual = module(inputs)
        torch.testing.assert_close(actual, expected, rtol=0, atol=0)
    values = torch.arange(256 * MIB // 4, dtype=torch.int32).remainder_(1009)
    writer = tensorfs.Writer(directory / "ordinary-replay.cozytensors")
    writer.component("next").put("data", values)
    checkpoint = writer.commit()
    region = memory.native.reserve(values.nbytes)
    read = region.prepare_read(
        checkpoint, [("next/data#value", 0, values.nbytes)], threads=1, reclaim=False
    )
    try:
        # The producer is admitted but unstarted: ordinary replay must still run.
        with torch.no_grad():
            held = module(inputs)
        torch.testing.assert_close(held, expected, rtol=0, atol=0)
        read.start()
        in_flight = issued(read)
        before = graphs.describe(module)
        with torch.no_grad():
            replayed = module(inputs)
        after = graphs.describe(module)
        assert after["replays"] > before["replays"], (before, after)
        done = read.wait()
        assert done["done"] and not done["canceled"]
        torch.testing.assert_close(replayed, expected, rtol=0, atol=0)
        use = region.prepare()
        copied = use.tensor.view(torch.int32).cpu()
        torch.testing.assert_close(copied, values, rtol=0, atol=0)
        del use
        return {
            "issued_before_cached_replay": in_flight,
            "producer": done,
            "before": before,
            "after": after,
            "sha256": hashlib.sha256(copied.numpy().tobytes()).hexdigest(),
        }
    finally:
        read.cancel()
        read.wait()
        graphs.release(torch, module)
        memory.trim()
        checkpoint.close()


def replay_allocator(memory, directory):
    from tensorfs import varena

    regions = [memory.native.reserve(16 * MIB) for _ in range(4)]
    for index, region in enumerate(regions):
        use = region.prepare()
        use.tensor.fill_(index + 1)
        use.commit()
        del use
    values = torch.full((16 * MIB,), 9, dtype=torch.uint8)
    writer = tensorfs.Writer(directory / "replay-producer.cozytensors")
    writer.component("next").put("data", values)
    checkpoint = writer.commit()
    future = memory.native.reserve(values.nbytes)
    with varena.replay(0):
        read = future.prepare_read(
            checkpoint, [("next/data#value", 0, values.nbytes)], reclaim=False
        )
        read.start()
        producer = read.wait()
        assert producer["done"] and not producer["canceled"]
        before = memory.native.stats()
        hook_before = varena.allocator_stats()
        try:
            torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
        except torch.OutOfMemoryError:
            pass
        else:
            raise AssertionError(
                "guarded allocation exceeded the managed admission limit"
            )
        after = memory.native.stats()
        hook_after = varena.allocator_stats()
        assert before["resident_bytes"] == 80 * MIB
        assert after["reclaimed_bytes"] == before["reclaimed_bytes"]
        assert hook_after["reclaimed_bytes"] == hook_before["reclaimed_bytes"]
        assert after["resident_bytes"] == before["resident_bytes"]
    activation = torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
    activation.fill_(71)
    assert activation[-1].item() == 71
    outside = memory.native.stats()
    assert outside["reclaimed_bytes"] - after["reclaimed_bytes"] >= 48 * MIB
    del activation
    torch.cuda.empty_cache()
    memory.trim()
    checkpoint.close()
    return {
        "managed_activation_limit": 224 * MIB,
        "producer_during_replay": producer,
        "before": before,
        "after_guarded_refusal": after,
        "after_unguarded_allocation": outside,
        "allocator_before": hook_before,
        "allocator_after_guarded_refusal": hook_after,
    }


def gil_cancellation(memory, directory, seed):
    del directory
    import threading
    from concurrent.futures import ThreadPoolExecutor

    from tensorfs import _ext, varena

    # Let CPython destroy the actual native guard under the GIL, without close()/GC/sleep.
    for replay in (False, True):
        guard = _ext._VarenaAllocatorCapture(0, replay=replay)
        del guard
        with varena.capture(0) if replay else varena.replay(0):
            pass
    loop, oracle = resident_loop(seed)
    region = memory.native.reserve(2 * MIB)
    rounds = []
    try:
        for index in range(3):
            use = region.prepare()
            use.tensor.fill_(index + 1)
            use.commit()
            del use
            running = loop.submit()
            first = progress(running)
            reclaim_entered, getter_entered = threading.Event(), threading.Event()

            def reclaim(entered=reclaim_entered):
                entered.set()
                return memory.native.reclaim(2 * MIB)

            def getter(entered=getter_entered):
                entered.set()
                return memory.native.stats()

            began = time.perf_counter_ns()
            with ThreadPoolExecutor(max_workers=2) as workers:
                freeing = workers.submit(reclaim)
                assert reclaim_entered.wait(5)
                time.sleep(0.02)
                assert not freeing.done(), "fixture did not enter a live context join"
                reading = workers.submit(getter)
                assert getter_entered.wait(5)
                time.sleep(0.02)
                queued_getter = not reading.done()
                requested = time.perf_counter_ns()
                running.cancel()
                (output,) = running.wait()
                ended = running.poll()
                released = freeing.result(timeout=10)
                observed = reading.result(timeout=10)
            torch.testing.assert_close(output, oracle, rtol=0, atol=0)
            assert queued_getter, (
                "getter was not waiting behind the native context join"
            )
            assert ended.done and ended.canceled and ended.completed_steps < 1500
            assert released >= 2 * MIB and observed["pinned_bytes"] == 0
            rounds.append(
                {
                    "first_steps": first.completed_steps,
                    "canceled_steps": ended.completed_steps,
                    "cancel_join_ms": (time.perf_counter_ns() - requested) / 1e6,
                    "whole_contention_ms": (time.perf_counter_ns() - began) / 1e6,
                    "reclaimed_bytes": released,
                    "getter_waited": queued_getter,
                }
            )
        # Last escaped storage alias must make physical pages reclaimable immediately.
        use = region.prepare()
        use.tensor.fill_(19)
        use.commit()
        alias = use.tensor[1:]
        del use
        began = time.perf_counter_ns()
        del alias
        released = memory.native.reclaim(2 * MIB)
        assert released >= 2 * MIB, memory.native.stats()
        return {
            "rounds": rounds,
            "finite_horizon_steps": 3000,
            "guard_drop_releases_immediately": True,
            "alias_drop_reclaimed_bytes": released,
            "alias_drop_to_reclaim_ms": (time.perf_counter_ns() - began) / 1e6,
        }
    finally:
        loop.close()
        memory.trim()


def exercise(seed, mode):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    from cozy_runtime.author._context import Device, place

    place(Device("cuda", torch.cuda.current_device()))
    memory = residency.Memory.open(
        "cuda:0", budget_bytes=(224 if mode == "prefetch-allocator" else 768) * MIB
    )
    assert memory.prefetch_supported
    with tempfile.TemporaryDirectory(prefix="cozy-prefetch-") as directory:
        if mode == "prefetch-pressure":
            result = pressure(memory, Path(directory), seed)
        elif mode == "prefetch-capture":
            result = capture_exclusion(memory, Path(directory))
        elif mode == "prefetch-replay":
            result = ordinary_replay(memory, Path(directory))
        elif mode == "prefetch-allocator":
            result = replay_allocator(memory, Path(directory))
        elif mode == "prefetch-gil":
            result = gil_cancellation(memory, Path(directory), seed)
        else:
            result = automatic(memory, Path(directory), seed, mode == "prefetch-cancel")
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
