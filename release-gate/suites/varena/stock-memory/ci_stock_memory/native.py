"""Bounded real-CUDA proof of the candidate native weight and Torch allocation bridge.

At most 256 MiB of test storage; no memory ballast, mocked driver, fault injection, package
reexecution, shared SDK installation, or allocator implementation replacement.
"""
import gc
import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import tensorfs
import torch
from tensorfs import varena
from tensorfs.varena import torch as vt

MIB = 1 << 20


def ready_pins() -> dict:
    """Ready-only borrowing uses real VMM storage and the ordinary DLPack owner."""
    arena = varena.Arena(0)
    region = arena.reserve(MIB)
    pointer = region.ptr
    cold = arena.stats()
    assert region.pin() is None
    assert arena.stats() == cold, "a cold pin miss allocated or changed residency"
    fill = region.prepare()
    assert fill.needs_fill
    loading = arena.stats()
    assert region.pin() is None
    assert arena.stats() == loading, "an uncommitted fill became readable"
    fill.tensor.fill_(11)
    fill.commit()
    del fill
    gc.collect()
    ready = arena.stats()
    use = region.pin()
    assert use is not None and not use.needs_fill
    assert use.tensor.data_ptr() == pointer and use.tensor[-1].item() == 11
    assert arena.stats()["fills"] == ready["fills"]
    alias = use.tensor[3:]
    del use
    gc.collect()
    assert arena.reclaim(MIB) == 0, "an escaped alias lost its borrowed physical pin"
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        result = alias[:65536].sum(dtype=torch.int64)
    del alias
    gc.collect()
    # Reclamation joins queued consumers before unmapping, regardless of their stream.
    assert arena.reclaim(MIB) >= MIB
    assert result.item() == 11 * 65536
    missing = arena.stats()
    assert missing["pinned_bytes"] == missing["resident_bytes"] == 0
    assert region.pin() is None and arena.stats() == missing
    fill = region.prepare()
    assert fill.needs_fill and fill.tensor.data_ptr() == pointer
    fill.tensor.fill_(17)
    fill.commit()
    del fill
    gc.collect()
    use = region.pin()
    assert use is not None and not use.needs_fill
    assert use.tensor[-1].item() == 17 and use.tensor.data_ptr() == pointer
    del use, result
    gc.collect()
    assert arena.reclaim(MIB) >= MIB
    final = arena.stats()
    assert final["pinned_bytes"] == final["resident_bytes"] == 0
    assert not final["poisoned"]
    return {
        "cold_and_loading_misses_unchanged": True,
        "escaped_alias_prevented_reclaim": True,
        "side_stream_result": 11 * 65536,
        "refilled_same_pointer": True,
        "final": final,
    }


def exercise() -> dict:
    torch.set_num_threads(1)
    vt.install(0)
    assert varena.PoisonedError.sticky is True
    assert varena.PoisonedError.__module__ == "tensorfs._ext"
    vt.set_limit(0, 224 * MIB)
    # Real driver argument validation, not a sticky device fault: preserve CUDA error typing.
    try:
        varena.Arena(32767)
    except torch.AcceleratorError as error:
        assert error.error_code == 101
    else:
        raise AssertionError("invalid CUDA ordinal did not preserve a typed driver error")
    arena = varena.Arena(0)
    regions = [arena.reserve(16 * MIB) for _ in range(4)]
    pointers = [region.ptr for region in regions]
    for index, region in enumerate(regions):
        use = region.prepare()
        assert use.needs_fill
        use.tensor.fill_(index + 1)
        use.commit()
        del use
    gc.collect()
    before = arena.stats()
    driver_before = torch.cuda.mem_get_info()[0]
    assert before["resident_bytes"] == 64 * MIB and before["pinned_bytes"] == 0, before
    live = regions[0].prepare()
    escaped = live.tensor[4:]
    del live
    gc.collect()
    assert arena.stats()["pinned_bytes"] == 16 * MIB

    # A Python worker reaches the same native working allocator without entering Python
    # from the allocator callback. The shared managed ceiling includes resident weights.
    def allocate():
        tensor = torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
        tensor.fill_(71)
        return tensor
    with ThreadPoolExecutor(max_workers=1) as executor:
        activation = executor.submit(allocate).result()
    torch.cuda.synchronize()
    after = arena.stats()
    driver_after = torch.cuda.mem_get_info()[0]
    allocation = vt.stats()
    assert after["resident_bytes"] <= 32 * MIB, after
    assert after["pinned_bytes"] == 16 * MIB, after
    assert after["reclaimed_bytes"] - before["reclaimed_bytes"] >= 32 * MIB, after
    assert activation[-1].item() == 71 and escaped[-1].item() == 1
    assert torch.cuda.memory_reserved() == allocation["mapped_bytes"] == 192 * MIB
    calls = allocation["driver_memory_calls"]
    del activation
    gc.collect()
    # Varena reuses the same default-stream block without a driver memory call.
    cached = torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
    assert vt.stats()["driver_memory_calls"] == calls
    del cached
    gc.collect()
    vt.trim()
    torch.cuda.synchronize()
    vt.trim()
    assert vt.stats()["mapped_bytes"] == 0
    del escaped
    gc.collect()

    refilled = 0
    for index, region in enumerate(regions):
        use = region.prepare()
        if use.needs_fill:
            refilled += 1
            use.tensor.fill_(index + 1)
            use.commit()
        assert use.tensor[-1].item() == index + 1
        assert use.tensor.data_ptr() == pointers[index]
        del use
    assert refilled >= 2

    abandoned = arena.reserve(4 * MIB)
    use = abandoned.prepare()
    survivor = use.tensor
    survivor.fill_(23)
    del use
    gc.collect()
    try:
        abandoned.prepare()
    except RuntimeError as error:
        assert "already being filled" in str(error)
    else:
        raise AssertionError("escaped incomplete fill was overwritten")
    del survivor
    gc.collect()
    use = abandoned.prepare()
    assert use.needs_fill
    use.tensor.fill_(29)
    use.commit()
    assert use.tensor[-1].item() == 29
    del use
    gc.collect()

    # Native guard covers all arenas and no-arena callers on this process/device.
    held = arena.stats()["resident_bytes"]
    with varena.capture(0):
        try:
            arena.reclaim(MIB)
        except RuntimeError as error:
            assert "capture" in str(error)
        else:
            raise AssertionError("reclaimed during capture guard")
    assert arena.stats()["resident_bytes"] == held

    # An allocation over the configured admission limit remains a typed Torch OOM.
    # There is no rerun of this program or its package work.
    try:
        torch.empty(512 * MIB, dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        pass
    else:
        raise AssertionError("managed activation admission limit was ignored")
    assert arena.stats()["pinned_bytes"] == 0
    vt.trim()
    torch.cuda.synchronize()
    vt.trim()
    vt.set_limit(0, None)
    use = regions[0].prepare()
    use.tensor.fill_(1)
    use.commit()
    del use
    gc.collect()
    # A request larger than device capacity gets a real CUDA OOM without filling VRAM.
    # Varena releases idle backing and makes only its bounded native allocation attempts.
    try:
        torch.empty(2 * torch.cuda.get_device_properties(0).total_memory, dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        pass
    else:
        raise AssertionError("oversized real CUDA allocation unexpectedly succeeded")
    assert vt.stats()["ooms"] >= 1
    assert arena.stats()["resident_bytes"] == 0
    graph_use = regions[0].prepare()
    graph_use.tensor.fill_(11)
    graph_use.commit()
    output = torch.empty(1024, dtype=torch.uint8, device="cuda")
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        torch.add(graph_use.tensor[:1024], 1, out=output)
    stream.synchronize()
    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph, stream=stream, reserve_bytes=2 * MIB):
        torch.add(graph_use.tensor[:1024], 1, out=output)
    # The graph owner retains its dependency Pin; reclaim can release other idle groups only.
    arena.reclaim(1 << 30)
    assert regions[0].resident
    vt.replay(graph)
    torch.cuda.synchronize()
    assert output[-1].item() == 12
    del graph, output, graph_use
    gc.collect()
    arena.reclaim(1 << 30)
    vt.trim()
    torch.cuda.synchronize()
    vt.trim()
    pin_proof = ready_pins()
    return {"passed": True, "torch": torch.__version__, "tensorfs": tensorfs.__version__,
            "before": before, "after_worker_allocation": after,
            "driver_free_before": driver_before, "driver_free_after": driver_after,
            "native_extension_sha256": hashlib.sha256(Path(tensorfs._ext.__file__).read_bytes()).hexdigest(), "allocation": allocation,
            "refilled_same_pointer": refilled, "ready_pin": pin_proof, "graph_capture_replay": True, "typed_invalid_device": True, "final_allocator": vt.stats(),
            "scope": "real CUDA native component; 224MiB managed physical ceiling; foreign CUDA overhead excluded"}


if __name__ == "__main__":
    print(json.dumps({"result": exercise()}, sort_keys=True), flush=True)
