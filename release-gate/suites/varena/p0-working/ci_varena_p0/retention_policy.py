"""Real native cache retention at two managed capacities, including a peer-room bound."""
from __future__ import annotations

import gc
import importlib.util
import json
import sys
from pathlib import Path

import tensorfs.varena.torch as vt
import torch
from tensorfs import varena

spec = importlib.util.spec_from_file_location("varena_retention_helpers", Path(__file__).with_name("probe.py"))
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
MIB = probe.MIB
BYTES = 4 * MIB


def touch(region, expected):
    use = region.prepare()
    tensor = None
    try:
        filled = use.needs_fill
        tensor = use.tensor
        pointer = tensor.data_ptr()
        use.consume_on(torch.cuda.current_stream().cuda_stream)
        if filled:
            tensor.fill_(expected)
            use.commit()
        torch.cuda.synchronize()
        assert int(tensor[0]) == int(tensor[-1]) == expected
        return {"filled": filled, "pointer": pointer}
    finally:
        tensor = use = None
        gc.collect()
        torch.cuda.synchronize()


def without_window(stats):
    return {key: value for key, value in stats.items() if key != "streaming_limit_bytes"}


def capacity_case(capacity, *, peer_limit=None, legacy_first=False):
    vt.set_limit(0, capacity)
    varena.make_room(0, peer_limit)
    arena = varena.Arena(0)
    assert BYTES % arena.granularity == 0, arena.granularity
    first, second = arena.reserve(BYTES), arena.reserve(BYTES)
    try:
        arena.set_streaming_limit(BYTES)
        initial = touch(first, 11)
        assert initial["filled"]
        if legacy_first:
            assert touch(second, 29)["filled"]
            legacy = arena.stats()
            assert not first.resident and second.resident, legacy
            assert legacy["resident_bytes"] == BYTES
            assert legacy["reclaimed_by_reason"]["streaming_limit"] == BYTES, legacy
        else:
            legacy = None
        before = arena.stats()
        assert before["pinned_bytes"] == 0, before
        # These are real native exclusion guards on an initialized CUDA device. The API
        # changes intent only; a reclaim/mapping attempt here would violate this boundary.
        with varena.capture(0):
            arena.clear_streaming_limit()
        with varena.replay(0):
            arena.clear_streaming_limit()
        after = arena.stats()
        assert without_window(before) == without_window(after), (before, after)
        assert first.preferred is False and second.preferred is False
        if legacy_first:
            refilled = touch(first, 11)
            assert refilled["filled"] and refilled["pointer"] == initial["pointer"]
            retained = touch(second, 29)
            assert not retained["filled"], retained
            final = arena.stats()
            assert first.resident and second.resident and final["resident_bytes"] == 2 * BYTES, final
            assert final["fills"] == 3 and final["evictions"] == before["evictions"], final
            assert final["reclaimed_by_reason"] == before["reclaimed_by_reason"], final
        else:
            loaded = touch(second, 29)
            assert loaded["filled"]
            final = arena.stats()
            assert not first.resident and second.resident and final["resident_bytes"] == BYTES, final
            assert final["fills"] == 2 and final["evictions"] == 1, final
            assert final["reclaimed_by_reason"]["streaming_limit"] == 0, final
            assert final["reclaimed_bytes"] == BYTES, final
        return {"managed_limit": capacity, "peer_limit": peer_limit, "legacy": legacy,
                "before_clear": before, "after_clear": after, "final": final,
                "scope": "actual managed-capacity/retention mechanism; not a smaller-GPU hardware result"}
    finally:
        varena.make_room(0, None)
        vt.set_limit(0, None)
        arena.reclaim(arena.stats()["resident_bytes"])
        assert arena.stats()["resident_bytes"] == 0
        del first, second, arena
        probe.clean()


CASES = {
    "roomy-retains-after-clear": lambda: capacity_case(2 * BYTES, legacy_first=True),
    "managed-capacity-still-reclaims": lambda: capacity_case(BYTES),
    "peer-capacity-still-reclaims": lambda: capacity_case(2 * BYTES, peer_limit=BYTES),
}


if __name__ == "__main__":
    try:
        probe.CASES = CASES
        probe.main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
