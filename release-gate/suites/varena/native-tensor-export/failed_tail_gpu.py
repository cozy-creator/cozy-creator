"""Actual capture refusal must retain typed storage custody, never force a context join.

Run in a separate GPU child: this intentionally leaves one export quarantined
until process exit. It does not qualify graceful physical reclamation.
"""

from __future__ import annotations

import argparse
import json
import time

import torch
from tensorfs.varena import Source, api
from tensorfs.varena import torch as vt


def run(device: int) -> dict:
    vt.install(device)
    stream = torch.cuda.Stream(device=device)
    gpu = api.open(device)
    source = Source.from_tensor(torch.arange(1024, dtype=torch.float32, device="cpu"))
    region = gpu.region(source)
    with gpu.use([region], stream.cuda_stream) as used:
        weight = used.tensor(dtype=source.dtype, shape=source.shape)
    with torch.cuda.stream(stream):
        copied = weight.clone()
        scratch = torch.zeros(1, device=torch.device("cuda", device))
    stream.synchronize()
    assert torch.equal(copied.cpu(), torch.arange(1024, dtype=torch.float32, device="cpu"))
    del copied
    region.close()
    # This case isolates consumer-tail refusal, so wait for the Source's own
    # checked producer/resource retirement before creating the CUDA graph.
    while api.retirement_stats()["queued"]:
        time.sleep(0.005)
    before = api.retirement_stats()
    resident = gpu.stats()["weights_resident"]
    assert resident > 0
    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph, stream=stream, reserve_bytes=0):
        scratch.add_(1)
        calls = vt.stats(device)["driver_memory_calls"]
        del weight  # Final foreign-storage owner; event recording is excluded here.
        assert vt.stats(device)["driver_memory_calls"] == calls
        during = api.retirement_stats()
        assert during["quarantined"] == before["quarantined"] + 1
        assert during["abandoned_cache_bytes"] == before["abandoned_cache_bytes"]
        assert gpu.stats()["weights_resident"] == resident
    stream.synchronize()
    graph.reset()
    del graph, scratch
    stream.synchronize()
    gpu.trim()
    # Native physical ownership remains even after capture and ordinary trim.
    # The old unpinned/context-join fallback would make these weight bytes evictable.
    assert gpu.stats()["weights_resident"] == resident
    after = api.retirement_stats()
    assert after["quarantined"] == before["quarantined"] + 1
    gpu.close()
    vt.retire_stream(stream, device)
    return {
        "passed": True,
        "scope": "intentional unproven storage quarantine; no reclamation claim",
        "device": device,
        "gpu_uuid": str(torch.cuda.get_device_properties(device).uuid),
        "retained_weight_bytes": resident,
        "before": before,
        "after": after,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", type=int, default=0)
    args = parser.parse_args()
    print(json.dumps(run(args.device), sort_keys=True))
