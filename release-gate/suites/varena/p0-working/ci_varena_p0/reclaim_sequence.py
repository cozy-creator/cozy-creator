"""Observe whether a completed working trim avoids an unnecessary weight eviction."""
from __future__ import annotations

import gc
import importlib.util
import json
from pathlib import Path
import sys
import time

import torch
from tensorfs.varena import _runtime as varena
import tensorfs.varena.torch as vt

spec = importlib.util.spec_from_file_location("varena_reclaim_helpers", Path(__file__).with_name("probe.py"))
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
MIB = probe.MIB


def measure():
    arena = varena.Arena(0)
    region = arena.reserve(8 * MIB)
    use = region.prepare()
    use.tensor.fill_(41)
    use.commit()
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use
    gc.collect()
    deadline = time.monotonic() + 10
    while arena.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, arena.stats()
        time.sleep(0.001)
    torch.cuda.synchronize()
    assert region.resident and arena.stats()["resident_bytes"] == 8 * MIB
    vt.set_limit(0, 24 * MIB)
    cached = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    cached.fill_(17)
    torch.cuda.synchronize()
    del cached
    before = {"working": vt.stats(0), "weights": arena.stats()}
    assert before["working"]["mapped_bytes"] == before["weights"]["resident_bytes"] == 8 * MIB, before
    value = torch.empty(16 * MIB, dtype=torch.uint8, device="cuda")
    value.fill_(23)
    torch.cuda.synchronize()
    assert int(value[0]) == int(value[-1]) == 23
    after = {"working": vt.stats(0), "weights": arena.stats()}
    retained = region.pin()
    byte_correct = False
    if retained is not None:
        byte_correct = int(retained.tensor[0]) == int(retained.tensor[-1]) == 41
        retained.consume_on(torch.cuda.current_stream().cuda_stream)
        del retained
    evicted = after["weights"]["reclaimed_by_reason"]["allocator"] - before["weights"]["reclaimed_by_reason"]["allocator"]
    preserved = (after["weights"]["resident_bytes"] == 8 * MIB and byte_correct
                 and after["weights"]["fills"] == before["weights"]["fills"] and evicted == 0)
    del value
    gc.collect()
    torch.cuda.synchronize()
    deadline = time.monotonic() + 10
    while arena.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, arena.stats()
        time.sleep(0.001)
    arena.reclaim(1 << 30)
    vt.set_limit(0, None)
    probe.clean()
    assert arena.stats()["resident_bytes"] == arena.stats()["pinned_bytes"] == 0
    return {"before": before, "after": after, "preserved_idle_weights": preserved,
            "weight_bytes_unchanged": byte_correct, "allocator_weight_eviction_bytes": evicted,
            "scope": "real allocation/output/retirement passed; preservation gate is evaluated separately by the App"}


if __name__ == "__main__":
    try:
        probe.CASES = {"working-before-weights": measure}
        probe.main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
