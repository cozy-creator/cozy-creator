"""One rule per case, on a real GPU. Each prints one JSON row; any failed assert fails the run."""
import gc
import json
import os
import sys
import tempfile
import threading
import time
from pathlib import Path

import torch

MODE, SELECTED = sys.argv[1], sys.argv[2]
os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "1"  # Runtime's setting
if MODE == "varena":
    import tensorfs
    from tensorfs import _ext
    from tensorfs.varena import _runtime as varena
    import tensorfs.varena.torch as vt

    vt.install(0)
torch.cuda.set_device(0)
MIB = 1 << 20
SIZES = (512, 513, 1536, 4096, 65536, MIB, MIB + 17, 8 * MIB)
COST = ("driver_memory_calls", "event_records", "event_queries", "event_waits", "context_joins")
CASES = {}
SLEEP_S = 1_500_000_000  # torch.cuda._sleep cycles: about a second of real GPU work


def case(function):
    CASES[function.__name__.replace("_", "-")] = function
    return function


def cap(managed, gpu=None):
    """A development cap leaving this much for our own bytes, the core's slack on top."""
    free, total = torch.cuda.mem_get_info(0)
    owned = vt.stats(0)["mapped_bytes"] + (gpu.stats()["resident_bytes"] if gpu else 0)
    vt.set_limit(0, 1 << 50)  # Any cap makes the core read the driver's reservation.
    reserved = vt.stats(0)["driver_reservation"]
    vt.set_limit(0, managed + max(total - free - owned, 0) + (128 << 20) + reserved)


def give_back():
    """Everything idle, through the one sequence, as a foreign program's pressure would."""
    vt.set_limit(0, 1)
    vt.set_limit(0, None)


def delta(before, after):
    return {key: after[key] - before[key] for key in COST}


def settle():
    gc.collect()
    torch.cuda.synchronize()
    vt.set_limit(0, None)
    torch.cuda.empty_cache()


def drained():
    """Every free working page back: its last fence is sealed, then seen done, by a later poll."""
    torch.cuda.synchronize()
    deadline = time.monotonic() + 10
    while True:
        torch.cuda.empty_cache()
        row = vt.stats(0)
        if not row["pending_bytes"] and not row["reclaimable_bytes"]:
            return
        assert time.monotonic() < deadline, row
        time.sleep(0.001)


def filled(gpu, bytes, value):
    region = gpu.reserve(bytes)
    use = region.prepare()
    use.tensor.fill_(value)
    use.commit()
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use
    gc.collect()
    deadline = time.monotonic() + 10
    while gpu.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, gpu.stats()
        time.sleep(0.001)
    torch.cuda.synchronize()
    return region


def touch(region):
    use = region.pin()
    assert use is not None
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use
    gc.collect()


def clear(gpu):
    gc.collect()
    torch.cuda.synchronize()
    vt.set_limit(0, None)
    give_back()


@case
def abandoned_fill():
    """Run 28306: a fill dropped uncommitted after its copy ran leaves no pin, only memory the
    next demand takes once the copy completes, with no context join, and fills again."""
    gpu = varena.Device(0)
    stream = torch.cuda.Stream()
    host = torch.full((32 * MIB,), 7, dtype=torch.uint8).pin_memory()
    regions = [gpu.reserve(host.numel()) for _ in range(4)]
    before = vt.stats(0)["context_joins"]
    for region in regions:
        use = region.prepare()
        assert use.needs_fill
        use.consume_on(stream.cuda_stream)
        with torch.cuda.stream(stream):
            use.tensor.copy_(host, non_blocking=True)
        del use
    gc.collect()
    deadline = time.monotonic() + 10
    while gpu.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, gpu.stats()
        time.sleep(0.001)
    assert gpu.stats()["resident_bytes"] == 4 * 32 * MIB, gpu.stats()
    stream.synchronize()
    cap(32 * MIB, gpu)
    value = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
    assert gpu.stats()["resident_bytes"] == 0, gpu.stats()
    assert vt.stats(0)["context_joins"] == before
    del value
    gc.collect()
    torch.cuda.synchronize()
    vt.set_limit(0, None)
    use = regions[0].prepare()
    assert use.needs_fill
    use.tensor.fill_(3)
    use.commit()
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use, regions
    clear(gpu)
    return {"abandoned_mib": 4 * 32}


@case
def cap_counts_the_driver():
    """A development cap reads as a smaller card: the driver's own charge comes out of it."""
    free, total = torch.cuda.mem_get_info(0)
    used = total - free
    vt.set_limit(0, 1 << 50)
    reserved = vt.stats(0)["driver_reservation"]
    limit = reserved + used + 512 * MIB
    vt.set_limit(0, limit)
    held = []
    try:
        while True:
            held.append(torch.empty(64 * MIB, dtype=torch.uint8, device="cuda").fill_(1))
    except torch.OutOfMemoryError:
        pass
    free, total = torch.cuda.mem_get_info(0)
    assert reserved and held and reserved + total - free <= limit, (len(held), reserved, total - free, limit)
    count = len(held)
    del held
    settle()
    return {"allocated_mib": count * 64}


@case
def fast_path():
    """A free never calls the driver, on an explicit stream, the production route."""
    stream = torch.cuda.Stream()
    allocate, release = torch._C._cuda_cudaCachingAllocator_raw_alloc, torch._C._cuda_cudaCachingAllocator_raw_delete
    handle = stream.cuda_stream

    def pairs(rounds):
        for _ in range(rounds):
            for size in SIZES:
                release(allocate(size, handle))

    pairs(512)
    if MODE == "varena":
        before = vt.stats(0)
    samples = []
    for _ in range(7):
        began = time.perf_counter_ns()
        pairs(20_000)
        samples.append((time.perf_counter_ns() - began) / (20_000 * len(SIZES)))
    row = {"ns_per_pair": sorted(samples)[3], "samples": samples}
    if MODE == "varena":
        spent = delta(before, vt.stats(0))
        # The cooperation service polls fences on its own clock, a handful of times in this.
        assert spent["driver_memory_calls"] == spent["event_waits"] == 0 and spent["event_records"] < 64, spent
        row["driver"] = spent
    stream.synchronize()
    return row


@case
def fences_bounded():
    """Thousands of frees on several streams record a handful of events, and reuse them."""
    streams = [torch.cuda.Stream() for _ in range(3)]
    before = vt.stats(0)
    for round in range(200):
        for stream in streams:
            with torch.cuda.stream(stream):
                held = [torch.empty(4096 * (1 + index % 7), dtype=torch.uint8, device="cuda").fill_(1) for index in range(50)]
                del held
        torch.empty((round + 3) * 2 * MIB, dtype=torch.uint8, device="cuda")  # a slow path polls
    after = vt.stats(0)
    spent = delta(before, after)
    assert after["event_handles"] <= 4 * (len(streams) + 2), after
    assert spent["event_records"] < after["frees"] - before["frees"], (spent, after["frees"] - before["frees"])
    settle()
    return {"frees": after["frees"] - before["frees"], "driver": spent, "event_handles": after["event_handles"]}


@case
def pending_working_before_weights():
    """Scratch still running is waited for; no weight is evicted while it can give the room."""
    gpu = varena.Device(0)
    region = filled(gpu, 8 * MIB, 41)
    a, b = torch.cuda.Stream(), torch.cuda.Stream()
    base = vt.stats(0)["mapped_bytes"]
    cap(base + 8 * MIB + 32 * MIB, gpu)
    with torch.cuda.stream(a):
        scratch = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
        torch.cuda._sleep(SLEEP_S)
        scratch.fill_(1)
        del scratch
    before, weights = vt.stats(0), gpu.stats()
    with torch.cuda.stream(b):
        began = time.monotonic()
        other = torch.empty(24 * MIB, dtype=torch.uint8, device="cuda")
        waited = time.monotonic() - began
        other.fill_(2)
    assert gpu.stats()["evictions"] == weights["evictions"] and region.resident, gpu.stats()
    spent = delta(before, vt.stats(0))
    assert spent["event_waits"] >= 1, spent
    use = region.pin()
    assert int(use.tensor[0]) == int(use.tensor[-1]) == 41
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use, other
    clear(gpu)
    return {"waited_s": waited, "driver": spent}


@case
def wait_stalls_nobody():
    """One thread's GPU wait holds no lock: other frees, slow paths and trims run through it."""
    a, b, c = torch.cuda.Stream(), torch.cuda.Stream(), torch.cuda.Stream()
    with torch.cuda.stream(c):
        warm = torch.empty(4096, dtype=torch.uint8, device="cuda")
        del warm
    torch.cuda.empty_cache()
    base = vt.stats(0)["mapped_bytes"]
    cap(base + 34 * MIB)
    with torch.cuda.stream(a):
        scratch = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
        torch.cuda._sleep(3 * SLEEP_S)
        scratch.fill_(1)
        del scratch
    done = {}

    def waiter():
        began = time.monotonic()
        with torch.cuda.stream(b):
            value = torch.empty(24 * MIB, dtype=torch.uint8, device="cuda")
            del value
        done["waited_s"] = time.monotonic() - began

    thread = threading.Thread(target=waiter)
    thread.start()
    time.sleep(0.2)
    worst = 0.0
    with torch.cuda.stream(c):
        while thread.is_alive():
            began = time.monotonic()
            value = torch.empty(4096, dtype=torch.uint8, device="cuda")
            del value
            torch.cuda.empty_cache()
            worst = max(worst, time.monotonic() - began)
    thread.join()
    assert done["waited_s"] > 0.5, done
    assert worst < 0.25, worst
    settle()
    return {**done, "worst_other_thread_s": worst}


def owner_wait(physical):
    held = []
    calls = []

    def shrink(bytes):
        time.sleep(0.2)  # a real owner takes time; the allocation must wait through it
        calls.append(bytes)
        held.clear()
        gc.collect()

    owner = varena.CacheOwner(0, shrink)
    held.append(torch.empty(512 * MIB, dtype=torch.uint8, device="cuda").fill_(3))
    torch.cuda.synchronize()
    filler = None
    if physical:
        free, _ = torch.cuda.mem_get_info(0)
        filler = torch.empty(free - 128 * MIB, dtype=torch.uint8, device="cuda").fill_(4)
    else:
        cap(vt.stats(0)["mapped_bytes"] + 16 * MIB)
    torch.cuda.synchronize()
    before = vt.stats(0)
    began = time.monotonic()
    value = torch.empty(384 * MIB, dtype=torch.uint8, device="cuda").fill_(5)
    waited = time.monotonic() - began
    after = vt.stats(0)
    assert calls and int(value[-1]) == 5, calls
    del value, filler, owner
    settle()
    return {"waited_s": waited, "owner_calls": len(calls), "physical_ooms": after["driver_physical_ooms"] - before["driver_physical_ooms"]}


@case
def owner_wait_physical():
    """A real card's shortage waits for an owner still releasing, as a software cap does."""
    return owner_wait(True)


@case
def owner_wait_capped():
    return owner_wait(False)


@case
def short_holders_wait():
    """Capture entry waits for short-lived holders; replay and reclaim run beside them."""
    calls = []

    def shrink(bytes):
        calls.append(bytes)
        time.sleep(0.3)

    owner = varena.CacheOwner(0, shrink)
    assert varena.request_cache_shrink(0, 2 * MIB) is not None
    deadline = time.monotonic() + 5
    while not calls:
        assert time.monotonic() < deadline
        time.sleep(0.001)
    with vt.replay_scope(0):  # beside the owner's callback
        value = torch.empty(16 * MIB, dtype=torch.uint8, device="cuda").fill_(1)  # may map
    guard = _ext._VarenaNoCapture(0)
    threading.Timer(0.3, guard.close).start()
    graph = torch.cuda.CUDAGraph()
    target = torch.zeros(16, dtype=torch.uint8, device="cuda")
    target.add_(1)
    torch.cuda.synchronize()
    began = time.monotonic()
    with vt.graph(graph, reserve_bytes=2 * MIB):
        target.add_(1)
    held = time.monotonic() - began
    assert held >= 0.25, held
    graph.reset()
    del value, owner, graph, target
    settle()
    return {"capture_entry_waited_s": held}


@case
def split_waits_and_caps():
    """A split waits for pending scratch, keeps to its cap and evicts outside the plan first."""
    gpu = varena.Device(0)
    stream = torch.cuda.Stream().cuda_stream
    base = vt.stats(0)["mapped_bytes"]
    other = torch.cuda.Stream()
    cap(base + 64 * MIB, gpu)
    with torch.cuda.stream(other):
        scratch = torch.empty(60 * MIB, dtype=torch.uint8, device="cuda")
        torch.cuda._sleep(SLEEP_S)
        scratch.fill_(1)
        del scratch
    selected = vt.split_select(0, "core/pending", [("only", [48 * MIB])], stream=stream)
    assert selected["choice_id"] == "only", selected
    settle()
    torch.cuda.synchronize()
    # A 96 MiB phase on a 128 MiB limit: each op gets at most a quarter of it, however much is free.
    weights = [filled(gpu, 24 * MIB, index) for index in range(4)]
    plan = varena.Plan(gpu, [("phase", weights)])
    plan.activate("phase")
    outside = filled(gpu, 30 * MIB, 9)
    cap(128 * MIB, gpu)
    menu = [("64", [64 * MIB]), ("16", [16 * MIB]), ("4", [4 * MIB])]
    selected = vt.split_select(0, "core/capped", menu, stream=stream)
    assert selected["choice_id"] == "16", selected
    assert not outside.resident and all(region.resident for region in weights), gpu.stats()
    plan.deactivate()
    clear(gpu)
    del weights, outside, plan
    return {"choice": selected["choice_id"]}


@case
def split_sample_is_live():
    """A split's measured peak is live bytes as requested, so neither how far the host runs ahead
    of the GPU nor a larger block best-fit hands out changes the level the next call picks."""
    stream = torch.cuda.Stream()
    key = "core/sample-split"
    with torch.cuda.stream(stream):
        spare = torch.empty(20 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
        del spare  # Free on this stream, within twice the size asked for below.
    token = vt.phase_enter(0, "core/sample")
    selected = vt.split_select(0, key, [("only", [16 * MIB])], stream=stream.cuda_stream)
    opened = vt.split_begin(0, key, "only", stream=stream.cuda_stream, completion_owner=token,
                            plan_generation=selected["plan_generation"])
    with torch.cuda.stream(stream):
        torch.cuda._sleep(SLEEP_S // 4)
        for mib in (16, 14, 12, 10):  # Distinct classes: none can reuse another's freed block.
            value = torch.empty(mib * MIB, dtype=torch.uint8, device="cuda").fill_(1)
            del value
    row = vt.split_end(0, opened, submitted=True)
    stream.synchronize()
    vt.phase_leave(0, token, completed=True)
    assert row["peak_bytes"] == 16 * MIB, row
    drained()
    return {"peak_mib": row["peak_bytes"] // MIB}


@case
def freed_pages_back_any_block():
    """Blocks freed around small live ones give back their pages, under a cap: one handle's pages
    come back only whole, so no small block may hold a large block's (run 29341)."""
    cap(1024 * MIB)
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        keep, large = [], []
        for size in (300, 200, 100, 180):
            large.append(torch.empty(size * MIB, dtype=torch.uint8, device="cuda").fill_(1))
            keep.append(torch.empty(2 * MIB, dtype=torch.uint8, device="cuda").fill_(1))
        del large  # 780 MiB freed, still running, between small blocks still live.
        keep += [torch.empty(90 * MIB, dtype=torch.uint8, device="cuda").fill_(2)]  # Under a third of the 300.
        big = torch.empty(800 * MIB, dtype=torch.uint8, device="cuda").fill_(3)
        assert int(big[0]) == int(big[-1]) == 3
    stats = vt.stats(0)
    del big, keep
    stream.synchronize()
    settle()
    # At the cap, with a large block just freed: a smaller one mustn't take a piece of it.
    cap(1024 * MIB)
    with torch.cuda.stream(stream):
        freed = torch.empty(640 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
        held = torch.empty(370 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
        del freed
        small = torch.empty(100 * MIB, dtype=torch.uint8, device="cuda").fill_(2)
        big = torch.empty(550 * MIB, dtype=torch.uint8, device="cuda").fill_(3)
        assert int(big[0]) == int(big[-1]) == 3 and int(small[-1]) == 2 and int(held[-1]) == 1
    del big, small, held
    stream.synchronize()
    settle()
    return {"mapped_mib": stats["mapped_bytes"] // MIB, "peak_mib": stats["mapped_peak"] // MIB}


@case
def eviction_order():
    """Outside the plan first, then the phase's most recently used; its next use stays."""
    gpu = varena.Device(0)
    a, b, c = (filled(gpu, 8 * MIB, index) for index in range(3))
    outside = filled(gpu, 8 * MIB, 3)
    plan = varena.Plan(gpu, [("phase", [a, b, c])])
    plan.activate("phase")
    touch(a)
    touch(b)
    torch.cuda.synchronize()
    resident = vt.stats(0)["mapped_bytes"] + gpu.stats()["resident_bytes"]
    cap(resident, gpu)
    first = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    assert not outside.resident and a.resident and b.resident and c.resident, gpu.stats()
    second = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    assert not b.resident and a.resident and c.resident, "phase's most recently used goes first"
    del first, second
    plan.deactivate()
    clear(gpu)
    return {"order": "outside, then most recently used"}


@case
def prefetch_rule():
    """Free >= bytes + the phase's remaining peak; evicts for it; unused prefetches go first."""
    key = "core/prefetch"
    token = vt.phase_enter(0, key)
    value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
    torch.cuda.synchronize()
    del value
    measured = vt.phase_leave(0, token, completed=True)
    assert measured["published"] and measured["peak_bytes"] == 8 * MIB, measured
    drained()
    with tempfile.TemporaryDirectory() as directory:
        writer = tensorfs.Writer(Path(directory) / "raw.cozytensors")
        source = torch.full((2 * MIB,), 41, dtype=torch.uint8)
        writer.component("net").put("bytes", source)
        checkpoint = writer.commit()
        parts = [("net/bytes#value", 0, source.nbytes)]
        gpu = varena.Device(0)
        idle = filled(gpu, 2 * MIB, 5)
        target = gpu.reserve(source.nbytes)
        before = vt.stats(0)
        # Nothing measured: only into room already free beside a quarter of the card, evicting nothing.
        cap(4 * MIB, gpu)
        assert target.prepare_prefetch(checkpoint, parts, threads=1) is None and idle.resident  # no phase
        token = vt.phase_enter(0, "core/unmeasured")
        assert target.prepare_prefetch(checkpoint, parts, threads=1) is None and idle.resident
        vt.phase_leave(0, token, completed=False)
        cap(64 * MIB, gpu)
        read = target.prepare_prefetch(checkpoint, parts, threads=1)
        assert read is not None and idle.resident
        read.cancel()
        read.wait()
        del read
        # 12 MiB of room: 2 MiB idle region, 8 MiB peak, 2 MiB read: the idle region may stay.
        cap(13 * MIB, gpu)
        token = vt.phase_enter(0, key)
        read = target.prepare_prefetch(checkpoint, parts, threads=1)
        assert read is not None and idle.resident
        read.start()
        read.wait()
        assert target.resident and gpu.stats()["prefetch_unused_bytes"] == 0
        # Working demand takes the unused prefetch before the older idle region.
        value = torch.empty(10 * MIB, dtype=torch.uint8, device="cuda").fill_(2)
        assert not target.resident and idle.resident, gpu.stats()
        assert gpu.stats()["prefetch_unused_bytes"] == 2 * MIB, gpu.stats()
        del value, read
        gc.collect()
        torch.cuda.synchronize()
        vt.phase_leave(0, token, completed=False)
        # A full card: an idle region gives way to the read, by the same rule.
        drained()
        cap(2 * MIB + 8 * MIB, gpu)
        token = vt.phase_enter(0, key)
        read = target.prepare_prefetch(checkpoint, parts, threads=1)
        assert read is not None and not idle.resident, gpu.stats()
        read.cancel()
        read.wait()
        del read
        vt.phase_leave(0, token, completed=False)
        after = vt.stats(0)
        clear(gpu)
        checkpoint.close()
    return {"attempts": after["prefetch_attempts"] - before["prefetch_attempts"],
            "admissions": after["prefetch_admissions"] - before["prefetch_admissions"]}


if MODE == "varena":
    warm = torch.empty(64 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
    torch.cuda._sleep(1000)
    warm.add_(1)
    int(warm.sum())
    torch.cuda.synchronize()
    del warm
    torch.cuda.empty_cache()

if MODE == "native":
    def report(name, row):
        print(json.dumps({"case": name, **row}), flush=True)
    report("fast-path", CASES["fast-path"]())
else:
    for name, function in CASES.items():
        if SELECTED in ("all", name):
            print(json.dumps({"case": name, **function()}), flush=True)
            torch.cuda.synchronize()
            row = vt.stats(0)
            print(json.dumps({"after": name, "mapped": row["mapped_bytes"], "pools": row["pools"], "event_handles": row["event_handles"]}), flush=True)
