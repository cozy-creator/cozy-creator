"""A real 24 MiB miss must not discard all 56 MiB of completed working cache."""
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

spec = importlib.util.spec_from_file_location("varena_bounded_trim_helpers", Path(__file__).with_name("probe.py"))
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
MIB = probe.MIB


def unpinned(arena):
    gc.collect()
    torch.cuda.synchronize()
    deadline = time.monotonic() + 10
    while arena.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, arena.stats()
        time.sleep(0.001)


def correct_bytes(tensor, expected):
    # Copy the complete actual result. Checking on CPU adds no temporary CUDA allocation
    # that could perturb the cache state under investigation.
    host = tensor.cpu()
    return bool(torch.all(host == expected).item())


def measure():
    arena = varena.Arena(0)
    arena.clear_streaming_limit()
    region = arena.reserve(8 * MIB)
    result = {"managed_limit_bytes": 64 * MIB, "request_bytes": 24 * MIB,
              "initial_cache_sizes": [8 * MIB, 16 * MIB, 32 * MIB]}
    value = use = weight = cached = None
    try:
        use = region.prepare()
        weight = use.tensor
        weight.fill_(41)
        use.commit()
        use.consume_on(torch.cuda.current_stream().cuda_stream)
        torch.cuda.synchronize()
        weight = use = None
        unpinned(arena)
        vt.set_limit(0, result["managed_limit_bytes"])

        # Keep all three allocations live until they exist, then prove their producer
        # stream drained and retire it. This creates globally reusable completed backing
        # without trimming it, polling timing assumptions, or inspecting segment order.
        producer = torch.cuda.Stream()
        with torch.cuda.stream(producer):
            cached = [torch.empty(n, dtype=torch.uint8, device="cuda")
                      for n in result["initial_cache_sizes"]]
            for index in range(len(cached)):
                cached[index].fill_(index + 11)
        producer.synchronize()
        cached = None
        gc.collect()
        producer.synchronize()
        vt.retire_stream(producer)
        before = {"working": vt.stats(0), "weights": arena.stats()}
        result["before"] = before
        assert before["working"]["allocated_bytes"] == 0, before
        assert before["working"]["mapped_bytes"] == before["working"]["free_bytes"] == 56 * MIB, before
        assert before["working"]["pending_bytes"] == 0, before
        assert before["weights"]["resident_bytes"] == 8 * MIB and before["weights"]["pinned_bytes"] == 0, before

        consumer = torch.cuda.Stream()
        with torch.cuda.stream(consumer):
            value = torch.empty(result["request_bytes"], dtype=torch.uint8, device="cuda")
            value.fill_(23)
        consumer.synchronize()
        result["while_live"] = {"working": vt.stats(0), "weights": arena.stats()}
        result["output_bytes_correct"] = correct_bytes(value, 23)
        use = region.pin()
        result["weight_bytes_correct"] = False
        if use is not None:
            weight = use.tensor
            use.consume_on(torch.cuda.current_stream().cuda_stream)
            result["weight_bytes_correct"] = correct_bytes(weight, 41)
        weight = use = None
        unpinned(arena)

        live = result["while_live"]
        result["weights_unchanged"] = (
            live["weights"]["resident_bytes"] == before["weights"]["resident_bytes"]
            and live["weights"]["fills"] == before["weights"]["fills"]
            and live["weights"]["evictions"] == before["weights"]["evictions"]
            and live["weights"]["reclaimed_by_reason"] == before["weights"]["reclaimed_by_reason"]
        )
        result["other_cache_preserved"] = live["working"]["free_bytes"] >= 24 * MIB
        assert live["working"]["allocated_bytes"] == result["request_bytes"], live
        assert result["output_bytes_correct"] and result["weight_bytes_correct"] and result["weights_unchanged"], result

        value = None
        gc.collect()
        consumer.synchronize()
        vt.retire_stream(consumer)
        result["after_free"] = {"working": vt.stats(0), "weights": arena.stats()}
        freed = result["after_free"]["working"]
        assert freed["allocated_bytes"] == freed["pending_bytes"] == 0, freed
        result["cache_after_free_preserved"] = freed["free_bytes"] >= 48 * MIB
        result["bounded_trim_passed"] = result["other_cache_preserved"] and result["cache_after_free_preserved"]
        result["scope"] = "single managed-capacity native allocation regression; no claim of full H3 cause or smaller-GPU qualification"
    finally:
        value = use = weight = cached = None
        unpinned(arena)
        vt.set_limit(0, None)
        arena.reclaim(arena.stats()["resident_bytes"])
        probe.clean()
        result["final"] = {"working": vt.stats(0), "weights": arena.stats()}
        probe.emit({"bounded_trim_measurement": result})
        assert result["final"]["weights"]["resident_bytes"] == result["final"]["weights"]["pinned_bytes"] == 0, result
        assert result["final"]["working"]["allocated_bytes"] == result["final"]["working"]["mapped_bytes"] == 0, result
    return result


if __name__ == "__main__":
    try:
        probe.CASES = {"bounded-working-trim": measure}
        probe.main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
