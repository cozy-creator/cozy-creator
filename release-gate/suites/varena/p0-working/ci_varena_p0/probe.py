"""Real CUDA memory stress, <=256 MiB of fixture-managed device storage, no model downloads.

Run through the adjacent Cozy App. This fresh process installs the allocator before CUDA init.
"""
from __future__ import annotations

import gc
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


CASES = {
    "empty-capture": empty_capture,
    "allocator-reuse": allocator_reuse,
    "side-stream": side_stream,
    "captured-reuse": captured_reuse,
    "retained-output": retained_output,
    "shared-pool": shared_pool,
    "distinct-pools": distinct_pools,
    "capture-exhaustion": capture_exhaustion,
    "replay-after-remap": replay_after_remap,
}


def main(config):
    installed = Path(tensorfs.__file__).resolve().parent
    actual = {
        "tensorfs_version": version("tensorfs"),
        "extension_sha256": digest(_ext.__file__),
        "shim_sha256": digest(installed / "_varena_torch.so"),
        "extension_path": str(Path(_ext.__file__).resolve()),
        "torch": torch.__version__, "torch_git": torch.version.git_version,
        "python": sys.version, "executable": sys.executable, "platform": platform.platform(),
    }
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
