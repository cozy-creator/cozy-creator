"""Real CUDA memory stress, <=256 MiB of fixture-managed device storage, no model downloads.

Run through the adjacent Cozy App. This fresh process installs the allocator before CUDA init.
"""
from __future__ import annotations

import gc
import ctypes
import hashlib
from importlib.metadata import version
import json
from pathlib import Path
import platform
import sys
import time

import tensorfs
from tensorfs import _ext, varena
import tensorfs.varena.torch as vt
import torch

MIB = 1 << 20
DRIVER = ("driver_memory_calls", "event_queries", "event_records", "event_waits")
MAX_MAPPED = 256 * MIB


def digest(path):
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def emit(value):
    print(json.dumps(value, sort_keys=True), flush=True)


def clean():
    gc.collect()
    torch.cuda.empty_cache()  # seal deferred frees; completion remains stream ordered
    torch.cuda.synchronize()
    torch.cuda.empty_cache()


def figures():
    row = vt.stats(0)
    assert row["mapped_bytes"] <= MAX_MAPPED, row
    assert row["mapped_bytes"] == sum(row[key] for key in (
        "allocated_capacity", "pending_bytes", "free_bytes", "unassigned_bytes"
    )), row
    return row


def capture(g, body, *, pool=None, stream=None, reserve=16 * MIB):
    pool = torch.cuda.graph_pool_handle() if pool is None else pool
    stream = torch.cuda.Stream() if stream is None else stream
    with vt.graph(g, pool=pool, stream=stream, reserve_bytes=reserve):
        before = figures()
        output = body()
        after = figures()
        assert all(after[key] == before[key] for key in DRIVER), (before, after)
    return output, pool, stream


def allocator_reuse():
    with torch.cuda.stream(torch.cuda.default_stream()):
        first = torch.empty(8192, dtype=torch.uint8, device="cuda")
        pointer = first.data_ptr()
        first.fill_(19)
        del first
        second = torch.empty(8192, dtype=torch.uint8, device="cuda")
        assert second.data_ptr() == pointer, "same-stream free did not reuse its size class"
        second.fill_(23)
        assert int(second[-1]) == 23
        del second
    tensors = [torch.full((n,), i + 1, dtype=torch.uint8, device="cuda")
               for i, n in enumerate((1, 512, 513, 768, 777, 1024, 4097, MIB + 17))]
    for i, tensor in enumerate(tensors):
        assert tensor.data_ptr() % 512 == 0, (tensor.numel(), tensor.data_ptr())
        assert int(tensor[-1]) == i + 1
    del tensor, tensors
    clean()
    row = figures()
    assert row["allocated_bytes"] == 0, row
    assert row["mapped_bytes"] == 0, row
    return {"aligned_sizes": 8, "same_stream_reused": True, "stats": row}


def default_deferred_free():
    with torch.cuda.stream(torch.cuda.default_stream()):
        data = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
        original = data.data_ptr()
        torch.cuda._sleep(10_000_000)
        data.fill_(7)
        total = data.sum(dtype=torch.int64)
        before = figures()
        del data
        replacement = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
        assert replacement.data_ptr() == original
        replacement.fill_(23)
        done = torch.cuda.Event()
        done.record()
        del replacement
        deferred = figures()
        assert all(deferred[key] == before[key] for key in DRIVER), (before, deferred)
    consumer = torch.cuda.Stream()
    with torch.cuda.stream(consumer):
        other = torch.full((8 * MIB,), 99, dtype=torch.uint8, device="cuda")
    if not done.query():
        assert other.data_ptr() != original, "unfenced default-stream block crossed streams"
    torch.cuda.synchronize()
    assert int(total) == 7 * 8 * MIB
    assert int(other[0]) == int(other[-1]) == 99
    del total, other, done, consumer
    clean()
    retired = figures()
    assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
    assert retired["event_handles"] == 0, retired
    return {"metadata_only_default_reuse": True, "cross_stream_fenced": True, "stats": retired}


def side_stream():
    producer = torch.cuda.current_stream()
    consumer = torch.cuda.Stream()
    data = torch.full((8 * MIB,), 7, dtype=torch.uint8, device="cuda")
    original = data.data_ptr()
    consumer.wait_stream(producer)
    with torch.cuda.stream(consumer):
        torch.cuda._sleep(2_000_000)
        result = data.sum(dtype=torch.int64)
    data.record_stream(consumer)
    event = torch.cuda.Event()
    event.record(consumer)
    del data
    replacement = torch.full((8 * MIB,), 99, dtype=torch.uint8, device="cuda")
    if not event.query():
        assert replacement.data_ptr() != original, "pending side-stream storage was reused"
    consumer.synchronize()
    assert int(result) == 7 * 8 * MIB
    del replacement, result
    clean()
    return {"sum": 7 * 8 * MIB, "stats": figures()}


def empty_capture():
    g = torch.cuda.CUDAGraph()
    stream = torch.cuda.Stream()
    event = torch.cuda.Event()
    event.record(stream)
    _, pool, _ = capture(g, lambda: event.record(stream), stream=stream, reserve=0)
    vt.replay(g, stream=stream)
    stream.synchronize()
    g.reset()
    del g, event
    clean()
    return {"empty_prepared_pool": list(pool), "stats": figures()}


def captured_reuse():
    g = torch.cuda.CUDAGraph()
    source = torch.full((MIB,), 6, dtype=torch.uint8, device="cuda")
    torch.cuda.synchronize()

    def body():
        # Cumulative allocation is 24 MiB, peak is 1 MiB. This must reuse within capture.
        for _ in range(24):
            temporary = torch.empty_like(source)
            temporary.copy_(source)
            del temporary
        return source + 5

    output, pool, _ = capture(g, body, reserve=8 * MIB)
    vt.replay(g)
    torch.cuda.synchronize()
    assert int(output[0]) == int(output[-1]) == 11
    g.reset()
    del g, source, output
    clean()
    row = figures()
    assert row["pools_bytes"] == 0, row
    return {"cumulative_bytes": 24 * MIB, "reservation_bytes": 8 * MIB, "stats": row}


def pool_variable_reuse():
    results = []
    for mode, output_bytes, expected in (("split", 8 * MIB, 29), ("coalesce", 16 * MIB, 43)):
        destination = torch.zeros(output_bytes, dtype=torch.uint8, device="cuda")
        graph = torch.cuda.CUDAGraph()
        body_facts = {}

        def body():
            before = figures()
            live = []

            def observe():
                live.append(figures()["allocated_capacity"] - before["allocated_capacity"])

            try:
                if mode == "split":
                    temporary = torch.empty(16 * MIB, dtype=torch.uint8, device="cuda")
                    temporary.fill_(13)
                    observe()
                    del temporary
                else:
                    first = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
                    second = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
                    first.fill_(11)
                    second.fill_(17)
                    observe()
                    del first, second
                temporary = torch.empty(output_bytes, dtype=torch.uint8, device="cuda")
                temporary.fill_(expected)
                destination.copy_(temporary)
                observe()
                del temporary
            finally:
                after = figures()
                body_facts["driver_delta"] = {key: after[key] - before[key] for key in DRIVER}
                body_facts["pool_live_peak_bytes"] = max(live, default=0)
                assert not any(body_facts["driver_delta"].values()), (before, after)

        result = {"mode": mode, "reservation_bytes": 16 * MIB}
        try:
            capture(graph, body, reserve=16 * MIB)
            vt.replay(graph)
            torch.cuda.synchronize()
            assert int(destination[0]) == int(destination[-1]) == expected
            result.update(passed=True, output_value=expected)
        except torch.OutOfMemoryError as error:
            # Record both independent arms without retrying either at a larger reservation.
            result.update(passed=False, error=str(error))
        finally:
            graph.reset()
            del graph, destination
            clean()
        retired = figures()
        assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
        assert retired["pools"] == retired["event_handles"] == 0, retired
        assert body_facts.get("pool_live_peak_bytes") == 16 * MIB, (result, body_facts)
        result.update(body_facts, retired=retired)
        results.append(result)
        emit({"pool_variable_reuse_arm": result})
    assert all(result["passed"] for result in results), json.dumps(results, sort_keys=True)
    return {"arms": results, "fixed_reservation_bytes": 16 * MIB}


def retained_output():
    g = torch.cuda.CUDAGraph()
    source = torch.full((MIB,), 4, dtype=torch.uint8, device="cuda")
    torch.cuda.synchronize()
    output, pool, capture_stream = capture(g, lambda: source + 9)
    replay_stream = torch.cuda.Stream()
    assert replay_stream.cuda_stream != capture_stream.cuda_stream
    vt.replay(g, stream=replay_stream)
    with torch.cuda.stream(replay_stream):
        copied = output.clone()
    output.record_stream(replay_stream)
    vt.record_pool_stream(pool, replay_stream)
    g.reset()
    del g
    torch.cuda.empty_cache()
    held = figures()
    assert held["pools_bytes"] > 0, "graph reset freed a still-live captured tensor"
    replay_stream.synchronize()
    assert int(output[0]) == int(copied[-1]) == 13
    del output, copied, source
    clean()
    retired = figures()
    assert retired["pools_bytes"] == 0, retired
    return {"held_pool_bytes": held["pools_bytes"], "different_replay_stream": True, "stats": retired}


def shared_pool():
    token = torch.cuda.graph_pool_handle()
    g1, g2 = torch.cuda.CUDAGraph(), torch.cuda.CUDAGraph()
    source = torch.full((MIB,), 2, dtype=torch.uint8, device="cuda")
    torch.cuda.synchronize()
    first, _, _ = capture(g1, lambda: source + 3, pool=token)
    second, _, _ = capture(g2, lambda: source + 7, pool=token)
    vt.replay(g1)
    copy1 = first.clone()
    vt.replay(g2)
    copy2 = second.clone()
    vt.record_pool_stream(token, torch.cuda.current_stream())
    torch.cuda.synchronize()
    assert int(copy1[0]) == 5 and int(copy2[-1]) == 9
    g1.reset()
    del g1, first, copy1
    clean()
    assert figures()["pools_bytes"] > 0, "first graph reset freed another graph's pool"
    vt.replay(g2)
    torch.cuda.synchronize()
    assert int(second[-1]) == 9
    g2.reset()
    del g2, second, copy2, source
    clean()
    assert figures()["pools_bytes"] == 0
    return {"shared_capture_order": True, "stats": figures()}


def distinct_pools():
    graphs = [torch.cuda.CUDAGraph(), torch.cuda.CUDAGraph()]
    streams = [torch.cuda.Stream(), torch.cuda.Stream()]
    source = torch.full((MIB,), 3, dtype=torch.uint8, device="cuda")
    torch.cuda.synchronize()
    outputs, tokens = [], []
    for i, g in enumerate(graphs):
        output, token, _ = capture(g, lambda i=i: source + i + 4)
        outputs.append(output)
        tokens.append(token)
    assert tokens[0] != tokens[1]
    for g, stream in zip(graphs, streams, strict=True):
        vt.replay(g, stream=stream)
    for stream in streams:
        stream.synchronize()
    assert int(outputs[0][0]) == 7 and int(outputs[1][-1]) == 8
    for g in graphs:
        g.reset()
    del output, outputs, graphs, g, source
    clean()
    assert figures()["pools_bytes"] == 0
    return {"concurrent_distinct_pools": True, "stats": figures()}


def capture_exhaustion():
    g = torch.cuda.CUDAGraph()
    token = torch.cuda.graph_pool_handle()
    stream = torch.cuda.Stream()
    reason = ""
    try:
        with vt.graph(g, pool=token, stream=stream, reserve_bytes=2 * MIB):
            before = figures()
            try:
                torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
            finally:
                after = figures()
                assert all(after[key] == before[key] for key in DRIVER), (before, after)
    except torch.OutOfMemoryError as error:
        reason = str(error)
    assert reason, "under-reserved capture unexpectedly succeeded"
    with torch.cuda.stream(stream):
        assert not torch.cuda.is_current_stream_capturing()
    # The owner already reset the failed graph; another reset is safe and must not release twice.
    g.reset()
    successor = torch.full((MIB,), 31, dtype=torch.uint8, device="cuda")
    assert int(successor[-1]) == 31
    del g, successor
    clean()
    assert figures()["pools_bytes"] == 0, figures()
    return {"typed_oom": reason, "successor_value": 31, "stats": figures()}


def replay_after_remap():
    arena = varena.Arena(0)
    region = arena.reserve(MIB)
    pointer = region.ptr
    use = region.prepare()
    use.tensor.fill_(17)
    use.commit()
    stream = torch.cuda.Stream()
    stream.wait_stream(torch.cuda.current_stream())
    use.consume_on(stream.cuda_stream)
    source = use.tensor
    g = torch.cuda.CUDAGraph()
    output, token, _ = capture(g, lambda: source + 5, stream=stream)
    vt.replay(g, stream=stream)
    stream.synchronize()
    assert int(output[-1]) == 22
    del source, use
    gc.collect()
    stream.synchronize()
    before = arena.stats()
    assert before["pinned_bytes"] == 0, before
    assert arena.reclaim(region.allocated_bytes) >= region.allocated_bytes
    evicted = arena.stats()
    assert evicted["resident_bytes"] == 0 and region.ptr == pointer, evicted
    for _ in range(3):
        use = region.prepare()
        assert use.needs_fill and use.tensor.data_ptr() == pointer
        use.tensor.fill_(17)
        use.commit()
        stream.wait_stream(torch.cuda.current_stream())
        use.consume_on(stream.cuda_stream)
        vt.replay(g, stream=stream)
        stream.synchronize()
        assert int(output[0]) == int(output[-1]) == 22
        del use
        gc.collect()
        stream.synchronize()
        assert arena.reclaim(region.allocated_bytes) >= region.allocated_bytes
    after = arena.stats()
    assert after["fills"] >= before["fills"] + 3
    assert after["evictions"] >= before["evictions"] + 4
    g.reset()
    del g, output, region, arena
    clean()
    return {"stable_pointer": pointer, "replays_after_refill": 3, "arena": after, "stats": figures()}


def rng_capture():
    seed = torch.initial_seed()
    custom = torch.Generator(device="cuda").manual_seed(seed + 1)
    torch.manual_seed(seed)
    expected = []
    for _ in range(2):
        expected.append([
            torch.rand(2048, dtype=torch.float32, device="cuda").cpu(),
            torch.rand(2048, dtype=torch.float32, device="cuda", generator=custom).cpu(),
        ])
    assert all(not torch.equal(a, b) for a, b in zip(expected[0], expected[1], strict=True))
    torch.manual_seed(seed)
    custom.manual_seed(seed + 1)
    clean()
    # No eager blocks remain to hide the first-RNG auxiliary-stream allocation path.
    assert figures()["allocated_bytes"] == figures()["mapped_bytes"] == 0
    g = torch.cuda.CUDAGraph()

    def body():
        return [
            torch.rand(2048, dtype=torch.float32, device="cuda"),
            torch.rand(2048, dtype=torch.float32, device="cuda", generator=custom),
        ]

    outputs, pool, _ = capture(g, body, reserve=8 * MIB)
    replay_stream = torch.cuda.Stream()
    digests = []
    for reference in expected:
        vt.replay(g, stream=replay_stream)
        with torch.cuda.stream(replay_stream):
            observed = [tensor.cpu() for tensor in outputs]
        vt.record_pool_stream(pool, replay_stream)
        assert all(torch.equal(a, b) for a, b in zip(observed, reference, strict=True))
        digests.append([hashlib.sha256(bytes(t.view(torch.uint8).tolist())).hexdigest() for t in observed])
    torch.manual_seed(seed)
    custom.manual_seed(seed + 1)
    vt.replay(g, stream=replay_stream)
    with torch.cuda.stream(replay_stream):
        observed = [tensor.cpu() for tensor in outputs]
    vt.record_pool_stream(pool, replay_stream)
    assert all(torch.equal(a, b) for a, b in zip(observed, expected[0], strict=True))
    g.reset()
    del g
    clean()
    assert figures()["pools_bytes"] > 0, "reset freed retained random output storage"
    assert all(torch.equal(t.cpu(), r) for t, r in zip(outputs, expected[0], strict=True))
    del outputs, observed, custom
    clean()
    retired = figures()
    assert retired["pools"] == retired["event_handles"] == 0, retired
    assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
    return {"default_and_custom_generators": True, "reseed_reproduced": True,
            "eager_replay_sha256": digests, "stats": retired}


def replay_hook_failure():
    arms = []
    for reset_in_hook in (False, True):
        source = torch.full((MIB,), 11, dtype=torch.uint8, device="cuda")
        destination = torch.zeros_like(source)
        torch.cuda.synchronize()
        graph = torch.cuda.CUDAGraph()

        def body():
            temporary = source + 17
            torch.cuda._sleep(100_000_000)
            destination.copy_(temporary)
            del temporary

        _, pool, capture_stream = capture(graph, body, reserve=8 * MIB)
        replay_stream = torch.cuda.Stream()
        assert replay_stream.cuda_stream != capture_stream.cuda_stream
        marker = torch.cuda.Event()
        fault = RuntimeError("P0 replay-end hook failed after launch")
        hook_state = {}

        def after_launch(active_graph):
            # This marker is ordered before the wrapper's required finally fence.
            marker.record(replay_stream)
            if reset_in_hook:
                active_graph.reset()
                torch.cuda.empty_cache()
                hook_state["retained_during_hook"] = figures()["pools_bytes"]
            raise fault

        registration = graph.register_replay_end_hook(after_launch)
        before = figures()
        caught = None
        try:
            vt.replay(graph, stream=replay_stream)
        except BaseException as error:
            caught = error
        finally:
            registration.remove()
        after = figures()
        # Drain before diagnosing a missing fence, so the failing fixture does not create a
        # second device fault by releasing a graph whose real work is still in flight.
        if caught is not fault or after["event_records"] <= before["event_records"]:
            replay_stream.synchronize()
            assert caught is fault, (type(caught).__name__, str(caught))
            raise AssertionError("replay-end hook error skipped the pool completion fence")
        if reset_in_hook and not hook_state.get("retained_during_hook"):
            replay_stream.synchronize()
            raise AssertionError("reset inside replay hook released the active replay's pool")
        graph.reset()
        del graph
        torch.cuda.empty_cache()
        held = figures()
        observed_pending = not marker.query()
        if observed_pending:
            assert held["pools_bytes"] > 0, "trim released an in-flight failed-hook replay"
        with torch.cuda.stream(replay_stream):
            copied = destination.clone()
        source.record_stream(replay_stream)
        destination.record_stream(replay_stream)
        replay_stream.synchronize()
        assert int(copied[0]) == int(copied[-1]) == 28
        del source, destination, copied, marker, registration
        clean()
        retired = figures()
        assert retired["pools"] == retired["event_handles"] == 0, retired
        assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
        arms.append({
            "reset_in_hook": reset_in_hook, "original_error_preserved": True,
            "replay_fence_records": after["event_records"] - before["event_records"],
            "retained_during_hook": hook_state.get("retained_during_hook"),
            "observed_pending_after_trim": observed_pending,
            "held_pool_bytes": held["pools_bytes"], "output_value": 28, "stats": retired,
        })
    return {"different_replay_stream": True, "arms": arms}


def dlpack_record_stream():
    consumer = torch.cuda.Stream()
    data = torch.full((MIB,), 3, dtype=torch.uint8, device="cuda")
    original = data.data_ptr()
    alias = torch.utils.dlpack.from_dlpack(data[17:])
    assert alias.untyped_storage().data_ptr() == original + 17
    consumer.wait_stream(torch.cuda.current_stream())
    with torch.cuda.stream(consumer):
        torch.cuda._sleep(10_000_000)
        total = alias.sum(dtype=torch.int64)
    alias.record_stream(consumer)
    done = torch.cuda.Event()
    done.record(consumer)
    del data, alias
    replacement = torch.full((MIB,), 99, dtype=torch.uint8, device="cuda")
    if not done.query():
        assert replacement.data_ptr() != original, "interior DLPack consumer was not recorded"
    consumer.synchronize()
    assert int(total) == 3 * (MIB - 17)
    del replacement, total, done
    clean()

    arena = varena.Arena(0)
    region = arena.reserve(MIB)
    use = region.prepare()
    use.tensor.fill_(29)
    use.commit()
    alias = use.tensor[17:]
    consumer.wait_stream(torch.cuda.current_stream())
    with torch.cuda.stream(consumer):
        total = alias.sum(dtype=torch.int64)
    # This storage is outside the working allocator's reservation. Torch's callback is a
    # compatibility no-op; the Region's own consumer declaration retains its physical backing.
    alias.record_stream(consumer)
    use.consume_on(consumer.cuda_stream)
    consumer.synchronize()
    assert int(total) == 29 * (MIB - 17)
    del alias, use, total
    gc.collect()
    assert arena.reclaim(region.allocated_bytes) >= region.allocated_bytes
    del region, arena, consumer
    clean()
    return {"interior_offset": 17, "working_sum": 3 * (MIB - 17),
            "foreign_region_sum": 29 * (MIB - 17), "stats": figures()}


def external_stream_retirement():
    arms = []
    for mode in ("none", "existing-consumer", "new-consumer"):
        handle = ctypes.c_void_p()
        status = torch.cuda.cudart().cudaStreamCreate(ctypes.addressof(handle))
        assert int(getattr(status, "value", status)) == 0, status
        external = torch.cuda.ExternalStream(handle.value, device=0)
        consumer = torch.cuda.Stream()
        with torch.cuda.stream(external):
            retained = torch.full((MIB,), 37, dtype=torch.uint8, device="cuda")
            temporary = torch.empty(8192, dtype=torch.uint8, device="cuda")
            temporary.fill_(1)
            del temporary
        original = retained.data_ptr()
        observed = None
        if mode == "existing-consumer":
            consumer.wait_stream(external)
            with torch.cuda.stream(consumer):
                torch.cuda._sleep(10_000_000)
                observed = retained.sum(dtype=torch.int64)
            retained.record_stream(consumer)
        external.synchronize()
        vt.retire_stream(external)
        status = torch.cuda.cudart().cudaStreamDestroy(handle.value)
        assert int(getattr(status, "value", status)) == 0, status
        if mode == "new-consumer":
            with torch.cuda.stream(consumer):
                torch.cuda._sleep(10_000_000)
                observed = retained.sum(dtype=torch.int64)
            retained.record_stream(consumer)
        if mode == "none":
            assert int(retained[0]) == 37
        done = torch.cuda.Event()
        done.record(consumer)
        before = figures()
        del retained
        after = figures()  # surfaces any destructor attempt to record the destroyed origin
        if mode == "none":
            assert after["event_records"] == before["event_records"], (before, after)
        replacement = torch.full((MIB,), 99, dtype=torch.uint8, device="cuda")
        if mode != "none" and not done.query():
            assert replacement.data_ptr() != original, "retirement erased a surviving consumer"
        consumer.synchronize()
        if observed is not None:
            assert int(observed) == 37 * MIB
        del observed, replacement, done, external, consumer
        clean()
        retired = figures()
        assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
        assert retired["event_handles"] == 0, retired
        arms.append({"mode": mode, "free_event_records": after["event_records"] - before["event_records"],
                     "stats": retired})
    return {"destroy_before_tensor_free": True, "arms": arms}


def cudnn_capture_shortfall():
    """A real cuDNN workspace refusal must abort capture before engine fallback/cache mutation."""
    import torch.nn.functional as functional

    assert torch.backends.cudnn.is_available() and torch.backends.cudnn.enabled
    assert not torch.backends.cudnn.benchmark
    stream = torch.cuda.Stream()
    generator = torch.Generator(device="cpu").manual_seed(1380214)
    trials = []
    chosen = None
    # All outputs occupy exactly one2MiB VMM page; inputs+weights stay below5MiB.
    for incoming, outgoing, edge in ((8, 32, 128), (32, 32, 128),
                                     (64, 32, 128), (32, 128, 64)):
        host_x = torch.randn((1, incoming, edge, edge), generator=generator) * 0.25
        host_w = torch.randn((outgoing, incoming, 3, 3), generator=generator) * 0.125
        with torch.cuda.stream(stream):
            x, weight = host_x.cuda(), host_w.cuda()
            warm = functional.conv2d(x, weight, padding=1)
            reference = warm.cpu()
        del warm
        clean()
        torch.cuda.reset_peak_memory_stats(0)
        before = figures()
        with torch.cuda.stream(stream):
            baseline = functional.conv2d(x, weight, padding=1)
            expected = baseline.cpu()
        measured = figures()
        assert measured["mapped_peak"] <= MAX_MAPPED, measured
        output_bytes = baseline.numel() * baseline.element_size()
        workspace_bytes = measured["allocated_peak"] - before["allocated_bytes"] - output_bytes
        assert output_bytes == 2 * MIB, output_bytes
        assert torch.equal(expected, reference), "baseline conv is not bitwise repeatable"
        trials.append({"shape": [1, incoming, edge, edge], "out_channels": outgoing,
                       "output_bytes": output_bytes, "workspace_peak": workspace_bytes,
                       "mapped_peak": measured["mapped_peak"]})
        del baseline, reference, host_x, host_w
        clean()
        if workspace_bytes > 0:
            chosen = (trials[-1], expected)
            break
        del x, weight, expected
        clean()
    assert chosen is not None, {"no_nonzero_workspace_candidate": trials}
    geometry, expected = chosen
    token = torch.cuda.graph_pool_handle()
    mapped = vt.prepare_pool(token, geometry["output_bytes"], device=0)
    assert mapped == geometry["output_bytes"], (mapped, geometry)
    graph = torch.cuda.CUDAGraph()
    captured = None
    capture_error = None
    translated_oom = False
    body_before = body_after = None
    try:
        with vt.graph(graph, pool=token, stream=stream, reserve_bytes=mapped):
            body_before = figures()
            try:
                captured = functional.conv2d(x, weight, padding=1)
            finally:
                body_after = figures()
                assert all(body_after[key] == body_before[key] for key in DRIVER), (body_before, body_after)
    except Exception as error:
        capture_error = f"{type(error).__name__}: {error}"
        translated_oom = isinstance(error, torch.OutOfMemoryError)
    # Exercise the eager successor even when old code wrongly caught OOM inside cuDNN.
    # Cleanup synchronizes first; a sticky device error aborts instead of proceeding.
    graph.reset()
    del graph, captured
    clean()
    cleanup = figures()
    torch.cuda.reset_peak_memory_stats(0)
    before = figures()
    with torch.cuda.stream(stream):
        eager = functional.conv2d(x, weight, padding=1)
        actual = eager.cpu()
    after = figures()
    assert after["mapped_peak"] <= MAX_MAPPED, after
    subsequent_workspace = after["allocated_peak"] - before["allocated_bytes"] - geometry["output_bytes"]
    parity = torch.equal(actual, expected)
    refusals = None if body_before is None or body_after is None else body_after["ooms"] - body_before["ooms"]
    report = {
        "trials": trials, "selected": geometry, "capture_error": capture_error,
        "translated_oom_after_cleanup": translated_oom, "capture_refusals": refusals,
        "capture_pool_bytes": mapped, "cleanup": cleanup,
        "eager_after_workspace_peak": subsequent_workspace, "eager_after_bitwise_equal": parity,
        "cudnn_version": torch.backends.cudnn.version(),
        "deterministic": torch.backends.cudnn.deterministic,
        "allow_tf32": torch.backends.cudnn.allow_tf32,
    }
    emit({"cudnn_capture_shortfall": report})
    del eager, actual, expected, chosen, x, weight
    clean()
    assert translated_oom, "capture must surface Torch OOM only after aborting, not finish with a fallback engine"
    assert refusals == 1, f"cuDNN retried workspace allocations inside capture: {refusals}"
    assert cleanup["pools"] == cleanup["pools_bytes"] == 0, cleanup
    assert parity, "failed capture changed the eager conv's exact output"
    assert subsequent_workspace == geometry["workspace_peak"], "failed capture changed the cached engine workspace"
    report["stats"] = figures()
    return report


CASES = {
    "empty-capture": empty_capture,
    "allocator-reuse": allocator_reuse,
    "default-deferred-free": default_deferred_free,
    "side-stream": side_stream,
    "captured-reuse": captured_reuse,
    "pool-variable-reuse": pool_variable_reuse,
    "retained-output": retained_output,
    "shared-pool": shared_pool,
    "distinct-pools": distinct_pools,
    "capture-exhaustion": capture_exhaustion,
    "replay-after-remap": replay_after_remap,
    "rng-capture": rng_capture,
    "replay-hook-failure": replay_hook_failure,
    "dlpack-record-stream": dlpack_record_stream,
    "external-stream-retirement": external_stream_retirement,
    "cudnn-capture-shortfall": cudnn_capture_shortfall,
}


def main(config):
    installed = Path(tensorfs.__file__).resolve().parent
    actual = {
        "tensorfs_version": version("tensorfs"),
        "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(installed / "_varena_torch.so"),
        "extension_path": str(Path(_ext.__file__).resolve()),
        "torch": torch.__version__, "torch_git": torch.version.git_version,
        "torch_distribution": version("torch"), "torch_cuda": torch.version.cuda,
        "torch_cxx11_abi": bool(torch._C._GLIBCXX_USE_CXX11_ABI),
        "python": sys.version, "executable": sys.executable, "platform": platform.platform(),
    }
    assert actual["torch"] == config["torch_version"], (config["torch_version"], actual["torch"])
    assert actual["torch_cuda"] == "13.0", actual
    assert actual["torch_distribution"] == "2.14.0", actual
    assert actual["torch_git"] == config["torch_git"], (config["torch_git"], actual["torch_git"])
    for key in ("tensorfs_version", "extension_sha256", "shim_sha256"):
        assert config[key] and actual[key] == config[key], (key, config[key], actual[key])
    assert not torch.cuda.is_initialized(), "child initialized CUDA before Varena installation"
    torch.set_num_threads(1)
    vt.install(device=0)
    torch.cuda.set_device(0)
    torch.manual_seed(config["seed"])
    actual["gpu"] = torch.cuda.get_device_name(0)
    actual["capability"] = list(torch.cuda.get_device_capability(0))
    actual["driver_free_total"] = list(torch.cuda.mem_get_info())
    emit({"provenance": actual})
    clean()
    initial = figures()
    emit({"initial_managed_memory": initial})
    assert initial["allocated_bytes"] == initial["mapped_bytes"] == 0, initial
    selected = list(CASES) if config["case"] == "all" else [config["case"]]
    results = {}
    for name in selected:
        assert name in CASES, f"unknown case {name}"
        clean()
        started = time.perf_counter()
        emit({"case": name, "state": "started"})
        result = CASES[name]()
        clean()
        retired = figures()
        assert retired["pools"] == retired["event_handles"] == 0, retired
        assert retired["allocated_bytes"] == retired["mapped_bytes"] == 0, retired
        result["retired"] = retired
        result["seconds"] = time.perf_counter() - started
        results[name] = result
        emit({"case": name, "state": "passed", "detail": result})
    emit({"result": {"passed": True, "provenance": actual, "cases": results, "final": figures()}})


if __name__ == "__main__":
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
