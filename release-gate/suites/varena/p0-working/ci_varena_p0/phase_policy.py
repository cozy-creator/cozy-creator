"""Real CUDA observations and raw-source admission, with no production policy cutover."""
from __future__ import annotations

from contextlib import contextmanager
import json
import importlib.util
from pathlib import Path
import sys
import tempfile
import time

import torch
import tensorfs
from tensorfs import varena
import tensorfs.varena.torch as vt

# The fresh child uses -I, so load the reviewed sibling by its exact frozen path.
_probe_spec = importlib.util.spec_from_file_location("varena_fixture_probe", Path(__file__).with_name("probe.py"))
assert _probe_spec is not None and _probe_spec.loader is not None
probe = importlib.util.module_from_spec(_probe_spec)
sys.modules[_probe_spec.name] = probe
_probe_spec.loader.exec_module(probe)

MIB = probe.MIB
DEVICE = 0
KEYS = set()


def key(name):
    value = "p2-real-cuda:" + name
    KEYS.add(value)
    return value


def allocated(*sizes):
    return [torch.empty(size, dtype=torch.uint8, device="cuda") for size in sizes]


def observe(name, sizes, *, entry=None, mode="eager", completed=True, invalidate=None):
    token = vt.phase_enter(DEVICE, entry, mode=mode)
    try:
        values = allocated(*sizes)
        for value in values:
            value.fill_(37)
        torch.cuda.synchronize()
        if invalidate:
            vt.phase_invalidate(DEVICE, token, invalidate)
    except BaseException:
        vt.phase_leave(DEVICE, token, completed=False, key=name)
        raise
    result = vt.phase_leave(DEVICE, token, completed=completed, key=name)
    del values, value
    probe.clean()
    return result


def retire_regions(arena):
    # Native DLPack final owners run on TensorFS's existing cleanup worker. Observe that
    # completion explicitly, as the P1 fixture does, instead of assuming Python gc joins it.
    deadline = time.monotonic() + 10
    while True:
        probe.clean()
        arena.reclaim(1 << 30)
        stats = arena.stats()
        if stats["resident_bytes"] == stats["pinned_bytes"] == 0:
            return stats
        assert time.monotonic() < deadline, stats
        time.sleep(0.01)


@contextmanager
def raw_source():
    with tempfile.TemporaryDirectory(prefix="varena-phase-source-") as directory:
        writer = tensorfs.Writer(Path(directory) / "raw.cozytensors")
        source = torch.full((2 * MIB,), 41, dtype=torch.uint8)
        writer.component("net").put("bytes", source)
        checkpoint = writer.commit()
        arena = varena.Arena(DEVICE)
        region = arena.reserve(source.nbytes)
        try:
            yield arena, region, checkpoint, [("net/bytes#value", 0, source.nbytes)], source
        finally:
            vt.set_limit(DEVICE, None)
            varena.make_room(DEVICE, None)
            retire_regions(arena)
            checkpoint.close()


def decline(arena, region, checkpoint, parts):
    before = vt.stats(DEVICE)
    before_weights = arena.stats()
    result = region.prepare_prefetch(checkpoint, parts, threads=1)
    after = vt.stats(DEVICE)
    after_weights = arena.stats()
    assert result is None and not region.resident, (result, after_weights)
    # allocated_bytes is the stable rounded reservation, even when no pages are mapped.
    assert after_weights["resident_bytes"] == before_weights["resident_bytes"] == 0, after_weights
    assert after["mapped_bytes"] == before["mapped_bytes"], (before, after)
    assert after["prefetch_admissions"] == before["prefetch_admissions"], (before, after)
    return {name: after[name] - before[name] for name in (
        "prefetch_attempts", "prefetch_admissions", "prefetch_declines", "driver_memory_calls"
    )}


def samples_and_invalidation():
    name = key("last-success")
    rounding = observe(key("alignment"), [513])
    # 513 requested bytes round to a 1024-byte class; every class is 512-byte aligned.
    assert rounding["peak_bytes"] == 1024 and rounding["backing_peak_bytes"] == 2 * MIB, rounding
    first = observe(name, [8 * MIB])
    second = observe(name, [4 * MIB])
    assert first["published"] and second["published"]
    assert first["peak_bytes"] == 8 * MIB and second["peak_bytes"] == 4 * MIB
    assert second["backing_peak_bytes"] == 4 * MIB
    snapshot = vt.phase_stats(DEVICE, name)
    assert snapshot["successful_passes"] == 2
    attempted = [
        observe(name, [12 * MIB], completed=False),
        observe(name, [16 * MIB], invalidate="caught_cut_retry"),
        observe(key("different-shape"), [6 * MIB], entry=name),
    ]
    assert all(not item["published"] for item in attempted)
    kept = vt.phase_stats(DEVICE, name)
    assert kept["peak_bytes"] == snapshot["peak_bytes"]
    assert kept["backing_peak_bytes"] == snapshot["backing_peak_bytes"]
    assert kept["successful_passes"] == snapshot["successful_passes"]
    return {"rounding": rounding, "first": first, "second": second,
            "rejected": attempted, "retained": kept}


def overlap_and_unknown():
    name = key("unknown-entry")
    with raw_source() as (arena, region, checkpoint, parts, _):
        token = vt.phase_enter(DEVICE)
        try:
            declined = decline(arena, region, checkpoint, parts)
            refused = False
            try:
                vt.phase_enter(DEVICE, name)
            except Exception as error:
                refused = "overlap" in str(error)
            assert refused, "a second execution scope must be rejected"
            busy = False
            try:
                vt.phase_forget(DEVICE, name)
            except Exception as error:
                busy = "already being filled" in str(error)
            assert busy, "unknown active key can still publish this owner; retirement must wait"
            value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
            torch.cuda.synchronize()
            result = vt.phase_leave(DEVICE, token, completed=True)
        except BaseException:
            vt.phase_leave(DEVICE, token, completed=False)
            raise
        del value
        probe.clean()
        assert not result["published"] and result["reason"] == "execution_key_unknown"
        assert not vt.phase_stats(DEVICE, name)["known"]
        return {"declined": declined, "overlap_rejected": refused, "owner_retirement_busy": busy,
                "unknown_result": result}


def pending_owned_demand():
    name = key("pending-owned")
    token = vt.phase_enter(DEVICE, name)
    value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    value.fill_(53)
    del value
    pending = vt.stats(DEVICE)
    assert pending["allocated_bytes"] == 0 and pending["pending_bytes"] == 8 * MIB, pending
    torch.cuda.reset_peak_memory_stats(DEVICE)
    reset = vt.stats(DEVICE)
    assert reset["allocated_peak"] == 0, reset
    torch.cuda.synchronize()
    result = vt.phase_leave(DEVICE, token, completed=True)
    assert result["peak_bytes"] == result["backing_peak_bytes"] == 8 * MIB, result
    probe.clean()
    return {"pending": pending, "global_peak_reset": reset["allocated_peak"], "result": result}


def nonexecution_and_native_replay():
    name = key("execution-modes")
    baseline = observe(name, [4 * MIB])
    modes = [observe(name, [6 * MIB], mode=mode) for mode in ("capture", "preparation", "replay")]
    assert all(not result["published"] for result in modes)
    token = vt.phase_enter(DEVICE, name)
    # Actual native replay exclusion is also used by PreparedSequence; it bypasses Torch replay hooks.
    with varena.replay(DEVICE):
        pass
    result = vt.phase_leave(DEVICE, token, completed=True)
    assert not result["published"] and result["reason"] == "capture_or_replay_attempt", result
    assert vt.phase_stats(DEVICE, name)["peak_bytes"] == baseline["peak_bytes"]
    return {"classified_modes": modes, "native_replay_guard": result}


def caught_allocator_failure():
    name = key("caught-oom")
    baseline = observe(name, [4 * MIB])
    vt.set_limit(DEVICE, 8 * MIB)
    token = vt.phase_enter(DEVICE, name)
    failed = False
    try:
        torch.empty(16 * MIB, dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        failed = True
    finally:
        result = vt.phase_leave(DEVICE, token, completed=True)
        vt.set_limit(DEVICE, None)
    assert failed and not result["published"] and result["invalidated"], result
    assert vt.phase_stats(DEVICE, name)["backing_peak_bytes"] == baseline["backing_peak_bytes"]
    return {"caught_typed_oom": failed, "result": result}


def allocator_reclaim_success():
    name = key("successful-reclaim")
    arena = varena.Arena(DEVICE)
    region = arena.reserve(4 * MIB)
    use = region.prepare()
    use.tensor.fill_(31)
    use.commit()
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    torch.cuda.synchronize()
    del use
    deadline = time.monotonic() + 10
    while arena.stats()["pinned_bytes"]:
        probe.clean()
        assert time.monotonic() < deadline, arena.stats()
        time.sleep(0.01)
    assert arena.stats()["resident_bytes"] == 4 * MIB
    vt.set_limit(DEVICE, 8 * MIB)
    token = vt.phase_enter(DEVICE, name)
    value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    value.fill_(61)
    torch.cuda.synchronize()
    result = vt.phase_leave(DEVICE, token, completed=True)
    assert result["published"] and result["peak_bytes"] == 8 * MIB, result
    assert arena.stats()["resident_bytes"] == 0, arena.stats()
    del value
    vt.set_limit(DEVICE, None)
    retire_regions(arena)
    return {"result": result, "weights_reclaimed_bytes": 4 * MIB}


def outside_live_reserve():
    name = key("outside-live")
    baseline = observe(name, [64 * MIB])
    with raw_source() as (arena, region, checkpoint, parts, _):
        vt.set_limit(DEVICE, 96 * MIB)
        outside = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
        outside.fill_(19)
        token = vt.phase_enter(DEVICE, name)
        declined = decline(arena, region, checkpoint, parts)
        value = torch.empty(64 * MIB, dtype=torch.uint8, device="cuda")
        value.fill_(23)
        torch.cuda.synchronize()
        result = vt.phase_leave(DEVICE, token, completed=True)
        assert result["peak_bytes"] == baseline["peak_bytes"] == 64 * MIB, result
        assert result["total_peak_bytes"] == 96 * MIB, result
        assert int(outside[-1]) == 19 and int(value[-1]) == 23
        del outside, value
        probe.clean()
        return {"prior": baseline, "declined": declined, "result": result}


def outside_pending_retirement():
    name = key("outside-pending")
    outside = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    outside.fill_(17)
    del outside  # Default stream retirement remains charged until checked completion.
    assert vt.stats(DEVICE)["pending_bytes"] >= 8 * MIB
    token = vt.phase_enter(DEVICE, name)
    probe.clean()  # Retiring a previous phase's pending allocation cannot subtract this phase's demand.
    value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
    value.fill_(29)
    torch.cuda.synchronize()
    result = vt.phase_leave(DEVICE, token, completed=True)
    assert result["peak_bytes"] == result["backing_peak_bytes"] == 4 * MIB, result
    assert result["total_peak_bytes"] == 8 * MIB, result
    del value
    probe.clean()
    # Reusing an outside pending address must retag it as a newly allocated phase block.
    outside = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    ptr = outside.data_ptr()
    del outside
    token = vt.phase_enter(DEVICE, name)
    reused = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    assert reused.data_ptr() == ptr
    torch.cuda.synchronize()
    retagged = vt.phase_leave(DEVICE, token, completed=True)
    assert retagged["peak_bytes"] == retagged["backing_peak_bytes"] == 8 * MIB, retagged
    del reused
    return {"retired_outside": result, "retagged_reuse": retagged}


def size_class_floor():
    name = key("size-class-floor")
    baseline = observe(name, [512, 1024])
    assert baseline["peak_bytes"] == 1536 and baseline["backing_peak_bytes"] == 4 * MIB, baseline
    with raw_source() as (arena, region, checkpoint, parts, _):
        vt.set_limit(DEVICE, 4 * MIB)
        token = vt.phase_enter(DEVICE, name)
        declined = decline(arena, region, checkpoint, parts)
        values = allocated(512, 1024)
        torch.cuda.synchronize()
        result = vt.phase_leave(DEVICE, token, completed=True)
        assert result["backing_peak_bytes"] == 4 * MIB, result
        del values
        probe.clean()
        return {"measured": baseline, "declined": declined, "repeat": result}


def outside_shared_slab():
    name = key("outside-shared-slab")
    baseline = observe(name, [512, 512])
    assert baseline["backing_peak_bytes"] == 2 * MIB, baseline
    with raw_source() as (arena, region, checkpoint, parts, _):
        vt.set_limit(DEVICE, 4 * MIB)
        outside = allocated(*([512] * ((2 * MIB // 512) - 1)))
        token = vt.phase_enter(DEVICE, name)
        first = torch.empty(512, dtype=torch.uint8, device="cuda")
        assert vt.stats(DEVICE)["mapped_bytes"] == 2 * MIB
        declined = decline(arena, region, checkpoint, parts)
        second = torch.empty(512, dtype=torch.uint8, device="cuda")
        assert vt.stats(DEVICE)["mapped_bytes"] == 4 * MIB
        torch.cuda.synchronize()
        result = vt.phase_leave(DEVICE, token, completed=True)
        assert result["peak_bytes"] == 1024 and result["backing_peak_bytes"] == 4 * MIB, result
        del outside, first, second
        probe.clean()
        return {"prior": baseline, "declined": declined, "repeat": result}


def admitted_read_and_cache_credit():
    name = key("raw-admitted")
    baseline = observe(name, [8 * MIB])
    with raw_source() as (arena, region, checkpoint, parts, source):
        vt.set_limit(DEVICE, 10 * MIB)
        token = vt.phase_enter(DEVICE, name)
        read = region.prepare_prefetch(checkpoint, parts, threads=1)
        assert read is not None and not region.resident
        read.start()
        receipt = read.wait()
        assert region.resident and arena.stats()["resident_bytes"] == 2 * MIB
        value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
        torch.cuda.synchronize()
        result = vt.phase_leave(DEVICE, token, completed=True)
        use = region.pin()
        assert use is not None and torch.equal(use.tensor.cpu(), source)
        # Holding a known ready weight must not have consumed the measured working reserve.
        use.consume_on(torch.cuda.current_stream().cuda_stream)
        del use, read, value
        retire_regions(arena)
        # A wholly completed eager segment is reusable backing, not an extra reserve.
        stream = torch.cuda.Stream()
        with torch.cuda.stream(stream):
            cached = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
            del cached
        stream.synchronize()
        # retire_stream proves the submitted free metadata before admission, without trimming.
        vt.retire_stream(stream)
        assert vt.stats(DEVICE)["reclaimable_bytes"] == 8 * MIB
        token = vt.phase_enter(DEVICE, name)
        read = region.prepare_prefetch(checkpoint, parts, threads=1)
        assert read is not None
        read.cancel()
        cancellation = read.wait()
        assert cancellation["done"] and cancellation["canceled"] and not cancellation["started"]
        assert cancellation["issued_bytes"] == 0 and not region.resident
        cancelled = vt.phase_leave(DEVICE, token, completed=False)
        del read, stream
        probe.clean()
        return {"measured": baseline, "read": receipt, "result": result,
                "completed_cache_admitted": True, "cancelled": cancelled}


def forget_samples():
    result = {}
    for name in sorted(KEYS):
        result[name] = vt.phase_forget(DEVICE, name)
        assert not vt.phase_stats(DEVICE, name)["known"]
    return {"retired_keys": result}


CASES = {
    "samples-and-invalidation": samples_and_invalidation,
    "overlap-and-unknown": overlap_and_unknown,
    "pending-owned-demand": pending_owned_demand,
    "nonexecution-and-native-replay": nonexecution_and_native_replay,
    "caught-allocator-failure": caught_allocator_failure,
    "allocator-reclaim-success": allocator_reclaim_success,
    "outside-live-reserve": outside_live_reserve,
    "outside-pending-retirement": outside_pending_retirement,
    "size-class-floor": size_class_floor,
    "outside-shared-slab": outside_shared_slab,
    "admitted-read-and-cache-credit": admitted_read_and_cache_credit,
    "forget-samples": forget_samples,
}


def main(config):
    # Reuse the established provenance/install/final-retirement harness, not a fake driver.
    probe.CASES = CASES
    probe.main(config)


if __name__ == "__main__":
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
