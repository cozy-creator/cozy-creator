"""One real unsplittable CUDA row must not evict an unrelated regenerable cache."""

import json
import os
import sys

import torch
from cozy_runtime._build_provenance import COMMIT
from cozy_runtime.author import Cut, _split, split
from cozy_runtime.internal import accel, execution, residency
from cozy_runtime.internal.execution import catch, cuts

MIB = 1 << 20


def exercise(mode):
    assert mode == "unsplittable"
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    memory = residency.Memory.open("cuda:0", budget_bytes=256 * MIB)
    memory.apply_budget()
    _split.install(execution)
    cache = [torch.full((64 * MIB,), 7, dtype=torch.uint8, device="cuda")]
    region = memory.region("split-unused-real-cache", "cache", release=cache.clear)
    value = torch.arange(64, device="cuda", dtype=torch.float32).reshape(1, 1, 64)
    try:
        with (
            memory.request("unsplittable-cache") as request,
            catch.serving(request),
            torch.no_grad(),
        ):
            before = request.stats()
            result = split(lambda x: x * 2 + 3, value, Cut(1))
            torch.cuda.synchronize()
            torch.testing.assert_close(result, value * 2 + 3, rtol=0, atol=0)
            events = catch.cut_rows(request)
            after = request.stats()
            assert len(events.sites) == 1 and events.sites[0].level == 0
            assert events.sites[0].need == 0, events.sites[0]
            assert len(cache) == 1, "unsplittable row reclaimed an unrelated cache"
            assert cache[0][0].item() == 7 and cache[0][-1].item() == 7
            assert not cuts._MEASURED, cuts._MEASURED
            return {
                "pid": os.getpid(),
                "runtime_source": COMMIT,
                "device": torch.cuda.get_device_name(),
                "local_cap_bytes": 256 * MIB,
                "exact_output": True,
                "unused_cache_retained_bytes": cache[0].numel(),
                "level": events.sites[0].level,
                "need": events.sites[0].need,
                "before": before,
                "after": after,
                "scope": "Real CUDA output and untouched regenerable cache at unsplittable floor.",
            }
    finally:
        cache.clear()
        del region
        _split.install(None)
        memory.trim()


if __name__ == "__main__":
    print(json.dumps(exercise(sys.argv[1]), sort_keys=True))
