"""File-backed CUDA weights and genuine allocator refusals under an executor-local cap."""

from __future__ import annotations

import importlib.metadata
import tempfile
from pathlib import Path

import tensorfs
import torch
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel, residency
from cozy_runtime.internal.execution import catch
from tensorfs import varena
from torch import nn

MIB = 1 << 20


def exercise(mode, seed, ctx):
    del ctx
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0", budget_bytes=256 * MIB)
    with tempfile.TemporaryDirectory(prefix="cozy-bounded-policy-") as directory:
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        width, count = 1024, 1 if mode in ("retry", "fallback") else 4
        names = ("a", "b", "c", "d") if mode in ("retry", "fallback") else ("a", "b")
        matrix = torch.eye(width)
        for name in names:
            for index in range(count):
                writer.component(name).put(f"{index}.weight", matrix)
        checkpoint = writer.commit()
        modules, models = {}, {}
        for name in names:
            with torch.device("meta"):
                module = nn.Sequential(
                    *(nn.Linear(width, width, bias=False) for _ in range(count))
                )
            vars(module)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, name),
                blocks=[str(i) for i in range(count)],
            )
            modules[name] = module
            models[name] = memory.load(module, "ram")
        inputs = torch.ones((2, width), device="cuda")
        try:
            if mode == "prefetch":
                result = prefetch(memory, modules, models, inputs, seed)
            elif mode == "bounded":
                result = bounded(memory, modules, models, inputs, seed)
            elif mode == "capture":
                result = capture_refusal(memory, seed)
            elif mode in ("retry", "fallback"):
                result = retry_recovery(memory, modules, models, inputs, seed, mode)
            else:
                raise ValueError(f"unknown mode: {mode}")
            return {
                "mode": mode,
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": importlib.metadata.version("tensorfs"),
                "device": torch.cuda.get_device_name(0),
                "local_memory_cap_bytes": 256 * MIB,
                **result,
            }
        finally:
            for model in models.values():
                model.close()
            memory.trim()
            checkpoint.close()


def prefetch(memory, modules, models, inputs, seed):
    with memory.request(
        f"policy-prefetch-{seed}", plan=[models["a"], models["b"]]
    ) as request:
        with torch.no_grad():
            torch.testing.assert_close(modules["a"](inputs), inputs, rtol=0, atol=0)
        a_regions = [block.region for block in models["a"].groups()]
        assert all(region.resident for region in a_regions)
        prior_started = request.prefetch_started
        prior_bytes = request.moved
        with torch.no_grad():
            torch.testing.assert_close(modules["b"](inputs), inputs, rtol=0, atol=0)
        assert request.prefetch_started > prior_started, request.stats()
        assert request.moved - prior_bytes == 16 * MIB, request.stats()
        assert all(region.resident for region in a_regions), (
            "lookahead evicted outgoing cached data"
        )
        prior_bytes = request.moved
        with torch.no_grad():
            torch.testing.assert_close(modules["a"](inputs), inputs, rtol=0, atol=0)
        assert request.moved == prior_bytes, request.stats()
        before_cancel = request.stats()

        # Make one next block genuinely absent and cancel its real source read.
        first, next_block = models["b"].blocks["0"], models["b"].blocks["1"]
        memory._discard(next_block, forget=True)
        with models["b"].passing(executing=True):
            memory._enter(first)
            try:
                memory._prefetch_next(first)
                pending = memory._prefetch
                assert pending is not None, request.stats()
                target = pending.region
                assert not target.preferred, (
                    "temporary admission preference escaped into I/O"
                )
                memory._settle_prefetch(cancel=True)
                assert memory._prefetch is None
                assert not target.preferred
                del target, pending
            finally:
                memory._exit(first)

        # Actually shorten the native prefix by reclaiming its idle preferred tail.
        del a_regions
        for block in models["a"].groups():
            memory._discard(block, forget=True)
        with torch.no_grad():
            modules["b"](inputs)
        # Region drop defers physical teardown. Remove retired outgoing handles first,
        # so the next real reclaim necessarily reaches B's preferred tail.
        memory.native.reclaim_unpreferred(int(memory.native.stats()["resident_bytes"]))
        assert all(block.region.preferred for block in models["b"].groups())
        first, next_block = models["b"].blocks["0"], models["b"].blocks["1"]
        with models["b"].passing():
            memory._enter(first)
            try:
                released = memory.native.reclaim(12 * MIB)
                assert released >= 12 * MIB
                assert not next_block.region.resident
                # Occupy the one streaming window with actual nonpreferred cached bytes.
                cached = memory.native.reserve(4 * MIB)
                fill = cached.prepare()
                fill.tensor.zero_()
                fill.commit()
                del fill
                torch.cuda.synchronize()
                assert cached.resident and not cached.preferred
                started, skipped = request.prefetch_started, request.prefetch_skipped
                before = memory.native.stats()
                memory._prefetch_next(first)
                assert memory._prefetch is None
                assert (
                    request.prefetch_started == started
                    and request.prefetch_skipped == skipped + 1
                )
                assert not next_block.region.preferred, (
                    "lookahead rebuilt a shortened prefix"
                )
                assert cached.resident and first.region.resident
                after = memory.native.stats()
                assert before["resident_bytes"] == after["resident_bytes"]
                del cached
            finally:
                memory._exit(first)
        return {
            "before_cancellation": before_cancel,
            "final_request": request.stats(),
            "outgoing_cached_weights_preserved": True,
            "return_to_outgoing_zero_reads": True,
            "cancellation_left_no_provisional_preference": True,
            "shortened_native_watermark_refused": True,
        }


def bounded(memory, modules, models, inputs, seed):
    with memory.request(f"policy-bounded-{seed}") as request, torch.no_grad():
        modules["a"](inputs)
        models["a"]._claim()
        first = models["a"].blocks["0"]
        memory._enter(first)
        alias = modules["a"][0].weight[:1]
        memory._exit(first)
        models["a"]._release()
        initial = memory.native.stats()["resident_bytes"]
        target = request.room().free + 4 * MIB
        reclaimed = request.reclaim(target)
        remaining = memory.native.stats()["resident_bytes"]
        assert reclaimed > 0 and initial - remaining == 4 * MIB, (
            initial,
            remaining,
            reclaimed,
        )
        torch.testing.assert_close(alias.cpu(), torch.eye(1024)[:1], rtol=0, atol=0)
        moved = request.moved
        torch.testing.assert_close(modules["a"](inputs), inputs, rtol=0, atol=0)
        assert request.moved - moved == 4 * MIB, request.stats()
        del alias
        return {
            "requested_free_bytes": target,
            "resident_bytes_before": initial,
            "resident_bytes_after": remaining,
            "reread_bytes": request.moved - moved,
            "alias_preserved": True,
            "scope": "real bounded reclamation mechanism; not an underestimated-workspace retry receipt",
        }


def capture_refusal(memory, seed):
    state = torch.zeros(1, device="cuda")
    stream = torch.cuda.Stream()
    current = torch.cuda.current_stream()
    stream.wait_stream(current)
    graph = torch.cuda.CUDAGraph()
    with memory.request(f"policy-capture-{seed}") as request, catch.serving(request):
        before = memory.native.stats()
        failure = None
        with varena.capture(0), torch.cuda.stream(stream):
            graph.capture_begin()
            try:
                catch.cut(
                    "policy-capture:impossible",
                    0,
                    lambda level, granularity: 0,
                    lambda level: torch.empty(
                        2 * torch.cuda.get_device_properties(0).total_memory,
                        dtype=torch.uint8,
                        device="cuda",
                    ),
                )
            except torch.OutOfMemoryError as error:
                failure = str(error)
            finally:
                try:
                    graph.capture_end()
                except RuntimeError:
                    if failure is None:
                        raise
        assert failure is not None, (
            "capture's impossible CUDA allocation was not refused"
        )
        stream.synchronize()
        graph.reset()
        after = memory.native.stats()
        assert before == after, (before, after)
        assert catch.cut_rows(request).sites == [], (
            "capture entered recovery/retry telemetry"
        )
        state.add_(1)
        assert state.item() == 1
        return {
            "capture_oom": failure,
            "native_state_unchanged": True,
            "healthy_after": True,
        }


def retry_recovery(memory, modules, models, inputs, seed, mode):
    plans, pools = {}, {}
    alias = None
    try:
        for name, model in models.items():
            with torch.inference_mode(False):
                state = inputs.clone()
            binding = model.retain()
            torch.cuda.empty_cache()
            before = torch.cuda.memory_reserved()

            def step(index, state=state, module=modules[name]):
                del index
                state.copy_(module(state))

            try:
                with (
                    memory.request(f"cache-graph-{seed}-{name}"),
                    binding.prepare(),
                    torch.no_grad(),
                ):
                    plans[name] = PreparedRepeat.capture(
                        step, state=(state,), steps=2, owners=(binding, modules[name])
                    )
                    if name == "a":
                        alias = modules[name][0].weight[:1]
            finally:
                binding.close()
            torch.cuda.empty_cache()
            pools[name] = max(torch.cuda.memory_reserved() - before, 0)

        assert alias is not None
        torch.cuda.synchronize()
        torch.cuda.empty_cache()
        native_before = memory.native.stats()
        allocator_before = varena.allocator_stats(0)
        assert native_before["pinned_bytes"] == 16 * MIB, native_before
        assert all(not plan.facts()["closed"] for plan in plans.values())
        # B is the first eligible idle owner; A stays active and its alias survives.
        # The small gap needs one weight block. The larger gap exceeds B's measured
        # graph-pool plus weight release, requiring the existing full fallback.
        gap = 4 * MIB if mode == "retry" else pools["b"] + 8 * MIB
        granularity = 2 * MIB
        gap = (gap + granularity - 1) // granularity * granularity
        limit = allocator_before["activation_admission_limit"]
        available = (
            limit
            - allocator_before["torch_reserved_bytes"]
            - native_before["resident_bytes"]
        )
        amount = (available + gap + granularity - 1) // granularity * granularity
        assert 0 < amount <= limit, (amount, limit, available, gap, pools)

        def operation(level):
            del level
            return torch.empty(amount, dtype=torch.uint8, device="cuda").fill_(7)

        with (
            memory.request(f"actual-oom-{seed}-{mode}") as request,
            models["a"].passing(),
            catch.serving(request),
        ):
            output = catch.cut(
                "policy:actual-capped-allocation",
                0,
                lambda level, granularity: 0,
                operation,
            )
            assert output[0].item() == output[-1].item() == 7
            rows = catch.cut_rows(request)
            assert len(rows.sites) == 1 and rows.sites[0].retries == (
                1 if mode == "retry" else 2
            ), rows
            closed = {
                name: bool(plan.facts()["closed"]) for name, plan in plans.items()
            }
            assert closed["b"] and not closed["a"], closed
            if mode == "retry":
                assert not closed["c"] and not closed["d"], closed
            else:
                assert closed["c"] and closed["d"], closed
            torch.testing.assert_close(alias.cpu(), torch.eye(1024)[:1], rtol=0, atol=0)
            native_after = memory.native.stats()
            allocator_after = varena.allocator_stats(0)
            result = {
                "allocation_bytes": amount,
                "deliberately_underestimated_need_bytes": 0,
                "managed_capacity_gap_bytes": gap,
                "measured_graph_pool_bytes": pools,
                "same_level_retries": rows.sites[0].retries,
                "closed_graphs": closed,
                "native_before": native_before,
                "native_after": native_after,
                "allocator_before": allocator_before,
                "allocator_after": allocator_after,
                "active_owner_and_alias_preserved": True,
            }
            del output
        return result
    finally:
        for plan in plans.values():
            plan.close()
        del alias
