"""Bounded real-CUDA proof of the candidate native weight and Torch allocation bridge.

At most 256 MiB of test storage; no memory ballast, mocked driver, fault injection, package
reexecution, shared SDK installation, or allocator implementation replacement.
"""
import gc
import hashlib
from pathlib import Path
import json
from concurrent.futures import ThreadPoolExecutor

import torch
import tensorfs
from tensorfs import varena

MIB = 1 << 20


def exercise() -> dict:
    torch.set_num_threads(1)
    varena.install_allocator()
    varena.set_activation_limit(0, 224 * MIB)
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

    # A Python worker has a separate native allocation thread. The normal caching allocator
    # requests its real backing segment through the patched import, below Python and the GIL.
    def allocate():
        tensor = torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
        tensor.fill_(71)
        return tensor
    with ThreadPoolExecutor(max_workers=1) as executor:
        activation = executor.submit(allocate).result()
    torch.cuda.synchronize()
    after = arena.stats()
    driver_after = torch.cuda.mem_get_info()[0]
    allocation = varena.allocator_stats()
    assert after["resident_bytes"] <= 32 * MIB, after
    assert after["pinned_bytes"] == 16 * MIB, after
    assert allocation["reclaimed_bytes"] >= 32 * MIB, allocation
    assert activation[-1].item() == 71 and escaped[-1].item() == 1
    assert torch.cuda.memory_reserved() == allocation["torch_reserved_bytes"] == 192 * MIB
    calls = allocation["allocation_calls"]
    del activation
    gc.collect()
    # Stock caching remains active: same-size allocation reuses the segment without native call.
    cached = torch.empty(192 * MIB, dtype=torch.uint8, device="cuda")
    assert varena.allocator_stats()["allocation_calls"] == calls
    del cached
    gc.collect()
    torch.cuda.empty_cache()
    assert varena.allocator_stats()["torch_reserved_bytes"] == 0
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
    torch.cuda.empty_cache()
    varena.set_activation_limit(0, None)
    use = regions[0].prepare()
    use.tensor.fill_(1)
    use.commit()
    del use
    gc.collect()
    # A request larger than device capacity gets a real CUDA OOM without filling VRAM.
    # The hook releases the idle group and retries only that CUDA call once.
    try:
        torch.empty(2 * torch.cuda.get_device_properties(0).total_memory, dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        pass
    else:
        raise AssertionError("oversized real CUDA allocation unexpectedly succeeded")
    assert varena.allocator_stats()["oom_retries"] >= 1
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
    with varena.capture(0):
        with torch.cuda.graph(graph, stream=stream):
            torch.add(graph_use.tensor[:1024], 1, out=output)
    # The graph owner retains its dependency Pin; reclaim can release other idle groups only.
    arena.reclaim(1 << 30)
    assert regions[0].resident
    with varena.capture(0):
        graph.replay()
    torch.cuda.synchronize()
    assert output[-1].item() == 12
    del graph, output, graph_use
    gc.collect()
    arena.reclaim(1 << 30)
    torch.cuda.empty_cache()
    return {"passed": True, "torch": torch.__version__, "tensorfs": tensorfs.__version__,
            "before": before, "after_worker_allocation": after,
            "driver_free_before": driver_before, "driver_free_after": driver_after,
            "native_extension_sha256": hashlib.sha256(Path(tensorfs._ext.__file__).read_bytes()).hexdigest(), "allocation": allocation,
            "refilled_same_pointer": refilled, "graph_capture_replay": True, "typed_invalid_device": True, "final_allocator": varena.allocator_stats(),
            "scope": "real CUDA native component; 224MiB activation admission limit; not a hard device cap"}


if __name__ == "__main__":
    print(json.dumps({"result": exercise()}, sort_keys=True), flush=True)
