"""One rule per case, on a real GPU. Each prints one JSON row; any failed assert fails the run."""
import gc
import json
import os
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path

import torch

from cozy_runtime.internal.contract import unsealed

#: The driver's view, read on purpose: this cell tests the memory manager itself.
mem_get_info: Callable[[int], tuple[int, int]] = unsealed(torch, "mem_get_info")

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
type JSON = None | bool | int | float | str | Sequence[JSON] | Mapping[str, JSON]
type Row = Mapping[str, JSON]  # what a case prints
CASES: dict[str, Callable[[], Row]] = {}
SLEEP_S = 1_500_000_000  # torch.cuda._sleep cycles: about a second of real GPU work


def case(function: Callable[[], Row]) -> Callable[[], Row]:
    CASES[function.__name__.replace("_", "-")] = function
    return function


def cap(managed: int, gpu: "varena.Device | None" = None) -> None:
    """A development cap leaving this much for our own bytes, the core's slack on top."""
    free, total = mem_get_info(0)
    owned = vt.stats(0)["mapped_bytes"] + (gpu.stats()["resident_bytes"] if gpu else 0)
    vt.set_limit(0, 1 << 50)  # Any cap makes the core read the driver's reservation.
    reserved = vt.stats(0)["driver_reservation"]
    vt.set_limit(0, managed + max(total - free - owned, 0) + (128 << 20) + reserved)


def give_back() -> None:
    """Everything idle, through the one sequence, as a foreign program's pressure would."""
    vt.set_limit(0, 1)
    vt.set_limit(0, None)


def delta(before: dict[str, int], after: dict[str, int]) -> dict[str, int]:
    return {key: after[key] - before[key] for key in COST}


def settle() -> None:
    gc.collect()
    torch.cuda.synchronize()
    vt.set_limit(0, None)
    torch.cuda.empty_cache()


def drained() -> None:
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


def pages() -> "varena.Pages":
    return varena.Pages(0, cap=0)


def filled(gpu: "varena.Device", bytes: int, value: int) -> "varena.Region":
    source = varena.Source(pages(), "core", 0, bytes, derive=lambda view: view.fill_(value) is None)
    region = gpu.region(bytes, source)
    use = region.use(torch.cuda.current_stream().cuda_stream)
    del use
    gc.collect()
    deadline = time.monotonic() + 10
    while gpu.stats()["pinned_bytes"]:
        assert time.monotonic() < deadline, gpu.stats()
        time.sleep(0.001)
    torch.cuda.synchronize()
    return region


def touch(region: "varena.Region") -> None:
    use = region.pin()
    assert use is not None
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use
    gc.collect()


def clear(gpu: "varena.Device") -> None:
    gc.collect()
    torch.cuda.synchronize()
    vt.set_limit(0, None)
    give_back()


@case
def abandoned_fill() -> Row:
    """Run 28306: a derive that raises after its copy ran leaves no pin, only memory the next
    demand takes once the copy completes, with no context join, and the Region fills again."""
    gpu = varena.Device(0)
    stream = torch.cuda.Stream()
    host = torch.full((32 * MIB,), 7, dtype=torch.uint8).pin_memory()
    failing = [True]

    def derive(view: torch.Tensor) -> bool:
        if failing[0]:
            view.copy_(host, non_blocking=True)
            raise InterruptedError("abandoned")
        view.fill_(3)
        return True

    regions = [gpu.region(host.numel(), varena.Source(pages(), "core", 0, host.numel(), derive=derive))
               for _ in range(4)]
    before = vt.stats(0)["context_joins"]
    with torch.cuda.stream(stream):
        for region in regions:
            try:
                region.use(stream.cuda_stream)
            except InterruptedError:
                continue
            raise AssertionError("a derive's error reaches its use")
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
    failing[0] = False
    use = regions[0].use(torch.cuda.current_stream().cuda_stream)
    del use, regions
    clear(gpu)
    return {"abandoned_mib": 4 * 32}


@case
def cap_counts_the_driver() -> Row:
    """A development cap reads as a smaller card: the driver's own charge comes out of it."""
    free, total = mem_get_info(0)
    used = total - free
    vt.set_limit(0, 1 << 50)
    reserved = vt.stats(0)["driver_reservation"]
    limit = reserved + used + 512 * MIB
    vt.set_limit(0, limit)
    held: list[torch.Tensor] = []
    try:
        while True:
            held.append(torch.empty(64 * MIB, dtype=torch.uint8, device="cuda").fill_(1))
    except torch.OutOfMemoryError:
        pass
    free, total = mem_get_info(0)
    assert reserved and held and reserved + total - free <= limit, (len(held), reserved, total - free, limit)
    count = len(held)
    del held
    settle()
    return {"allocated_mib": count * 64}


@case
def fast_path() -> Row:
    """A free never calls the driver, on an explicit stream, the production route."""
    stream = torch.cuda.Stream()
    allocate, release = torch._C._cuda_cudaCachingAllocator_raw_alloc, torch._C._cuda_cudaCachingAllocator_raw_delete
    handle = stream.cuda_stream

    def pairs(rounds: int) -> None:
        for _ in range(rounds):
            for size in SIZES:
                release(allocate(size, handle))

    pairs(512)
    if MODE == "varena":
        before = vt.stats(0)
    samples: list[float] = []
    for _ in range(7):
        began = time.perf_counter_ns()
        pairs(20_000)
        samples.append((time.perf_counter_ns() - began) / (20_000 * len(SIZES)))
    row: dict[str, JSON] = {"ns_per_pair": sorted(samples)[3], "samples": samples}
    if MODE == "varena":
        spent = delta(before, vt.stats(0))
        # The cooperation service polls fences on its own clock, a handful of times in this.
        assert spent["driver_memory_calls"] == spent["event_waits"] == 0 and spent["event_records"] < 64, spent
        row["driver"] = spent
    stream.synchronize()
    return row


@case
def fences_bounded() -> Row:
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
def pending_working_before_weights() -> Row:
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
    assert use is not None
    assert int(use.tensor[0]) == int(use.tensor[-1]) == 41
    use.consume_on(torch.cuda.current_stream().cuda_stream)
    del use, other
    clear(gpu)
    return {"waited_s": waited, "driver": spent}


@case
def wait_stalls_nobody() -> Row:
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
    done: dict[str, float] = {}

    def waiter() -> None:
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


def owner_wait(physical: bool) -> Row:
    held: list[torch.Tensor] = []
    calls: list[int] = []

    def shrink(bytes: int) -> None:
        time.sleep(0.2)  # a real owner takes time; the allocation must wait through it
        calls.append(bytes)
        held.clear()
        gc.collect()

    owner = varena.CacheOwner(0, shrink)
    held.append(torch.empty(512 * MIB, dtype=torch.uint8, device="cuda").fill_(3))
    torch.cuda.synchronize()
    filler = None
    if physical:
        free, _ = mem_get_info(0)
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
def owner_wait_physical() -> Row:
    """A real card's shortage waits for an owner still releasing, as a software cap does."""
    return owner_wait(True)


@case
def owner_wait_capped() -> Row:
    return owner_wait(False)


@case
def short_holders_wait() -> Row:
    """Capture entry waits for short-lived holders; replay and reclaim run beside them."""
    calls: list[int] = []

    def shrink(bytes: int) -> None:
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
def split_waits_and_caps() -> Row:
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
def split_sample_is_live() -> Row:
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
def freed_pages_back_any_block() -> Row:
    """Blocks freed around small live ones give back their pages, under a cap: one handle's pages
    come back only whole, so no small block may hold a large block's (run 29341)."""
    cap(1024 * MIB)
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        keep: list[torch.Tensor] = []
        large: list[torch.Tensor] = []
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
        held = torch.empty(360 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
        del freed
        small = torch.empty(100 * MIB, dtype=torch.uint8, device="cuda").fill_(2)
        big = torch.empty(545 * MIB, dtype=torch.uint8, device="cuda").fill_(3)
        assert int(big[0]) == int(big[-1]) == 3 and int(small[-1]) == 2 and int(held[-1]) == 1
    del big, small, held
    stream.synchronize()
    settle()
    return {"mapped_mib": stats["mapped_bytes"] // MIB, "peak_mib": stats["mapped_peak"] // MIB}


@case
def fill_keeps_phase_pages() -> Row:
    """A weight's demand leaves the free pages this phase will use again, and takes another weight;
    with no weight left to give, it has them too."""
    gpu = varena.Device(0)
    key = "core/keeps"
    token = vt.phase_enter(0, key)
    value = torch.empty(64 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
    torch.cuda.synchronize()
    del value
    assert vt.phase_leave(0, token, completed=True)["published"]
    torch.cuda.synchronize()
    idle = filled(gpu, 16 * MIB, 3)
    mapped = vt.stats(0)["mapped_bytes"]
    cap(mapped + 16 * MIB, gpu)
    token = vt.phase_enter(0, key)
    wanted = filled(gpu, 16 * MIB, 4)
    assert not idle.resident and wanted.resident and vt.stats(0)["mapped_bytes"] == mapped, (gpu.stats(), vt.stats(0))
    last = filled(gpu, 32 * MIB, 5)
    assert last.resident and vt.stats(0)["mapped_bytes"] < mapped, vt.stats(0)
    vt.phase_leave(0, token, completed=False)
    del idle, wanted, last
    clear(gpu)
    return {"kept_mib": mapped // MIB}


@case
def pool_use_leaves_pass_measured() -> Row:
    """An LM's tokens replay inside its eager pass: a use of a recorded pool doesn't void it."""
    pool = torch.cuda.graph_pool_handle()
    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph, pool=pool, reserve_bytes=16 * MIB):
        torch.empty(8 * MIB, dtype=torch.uint8, device="cuda").fill_(2)
    token = vt.phase_enter(0, "core/pool")
    vt.prepare_pool(pool, 0, 0)
    vt.record_pool_stream(pool, torch.cuda.current_stream(0), 0)
    assert vt.phase_leave(0, token, completed=True)["published"]
    graph.reset()
    settle()
    return {}


@case
def eviction_order() -> Row:
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
    assert not b.resident and a.resident and c.resident, "phase's most recently used goes first"  # type: ignore[unreachable]  # `resident` is a live property; mypy keeps the narrowing above
    del first, second  # type: ignore[unreachable]  # the same
    plan.deactivate()
    clear(gpu)
    return {"order": "outside, then most recently used"}


@case
def prefetch_rule() -> Row:
    """The core alone fills ahead, what the Plan says comes next: free >= bytes + the phase's
    remaining peak; it evicts for that; unused fills go first; a demand fill finds the bytes."""
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
        gpu = varena.Device(0)
        tier = pages()
        stream = torch.cuda.current_stream().cuda_stream
        idle, anchor = filled(gpu, 2 * MIB, 5), filled(gpu, 2 * MIB, 6)

        def ahead() -> "tuple[varena.Region, varena.Plan]":
            """A use of `anchor`, then what the core did about the Region after it."""
            target = gpu.region(source.nbytes, varena.Source(
                tier, "core", 1, source.nbytes, checkpoint=checkpoint,
                parts=[("net/bytes#value", 0, source.nbytes)],
            ))
            plan = varena.Plan(gpu, [("phase", [anchor, target])])
            plan.activate("phase")
            attempts = vt.stats(0)["prefetch_attempts"]
            used = anchor.use(stream)
            deadline = time.monotonic() + 10
            while vt.stats(0)["prefetch_attempts"] == attempts:
                assert time.monotonic() < deadline, "the core offered no fill"
                time.sleep(0.001)
            while vt.stats(0)["prefetch_declines"] == declines[0] and not target.resident:
                assert time.monotonic() < deadline, gpu.stats()
                time.sleep(0.001)
            declines[0] = vt.stats(0)["prefetch_declines"]
            del used
            gc.collect()
            plan.deactivate()
            return target, plan

        declines = [vt.stats(0)["prefetch_declines"]]
        before = vt.stats(0)
        # Nothing measured: only into room already free beside a quarter of the card, evicting nothing.
        cap(6 * MIB, gpu)
        target, plan = ahead()  # no phase
        assert not target.resident and idle.resident, gpu.stats()
        token = vt.phase_enter(0, "core/unmeasured")
        target, plan = ahead()
        assert not target.resident and idle.resident, gpu.stats()
        vt.phase_leave(0, token, completed=False)
        cap(64 * MIB, gpu)
        target, plan = ahead()
        assert target.resident and idle.resident, gpu.stats()
        del target, plan
        gc.collect()
        drained()
        # 14 MiB of room: 2 MiB anchor, 2 MiB idle region, 8 MiB peak, 2 MiB read: both stay.
        cap(15 * MIB, gpu)
        token = vt.phase_enter(0, key)
        target, plan = ahead()
        assert target.resident and idle.resident, gpu.stats()
        unused = gpu.stats()["prefetch_unused_bytes"]
        # Working demand takes the unused fill before the older idle region.
        value = torch.empty(10 * MIB, dtype=torch.uint8, device="cuda").fill_(2)
        assert not target.resident and idle.resident, gpu.stats()  # type: ignore[unreachable]  # `resident` is a live property; mypy keeps the narrowing above
        assert gpu.stats()["prefetch_unused_bytes"] == unused + 2 * MIB, gpu.stats()  # type: ignore[unreachable]  # the same
        del value, target, plan
        gc.collect()
        torch.cuda.synchronize()
        vt.phase_leave(0, token, completed=False)
        # A full card: an idle region gives way to the read, by the same rule.
        drained()
        cap(2 * MIB + 2 * MIB + 8 * MIB, gpu)
        token = vt.phase_enter(0, key)
        target, plan = ahead()
        assert target.resident and not idle.resident, gpu.stats()
        used = target.use(stream)
        assert int(used.tensor[source.nbytes - 1]) == 41 and bool((used.tensor == 41).all())
        del used, target, plan
        vt.phase_leave(0, token, completed=False)
        after = vt.stats(0)
        clear(gpu)
        checkpoint.close()
    return {"attempts": after["prefetch_attempts"] - before["prefetch_attempts"],
            "admissions": after["prefetch_admissions"] - before["prefetch_admissions"]}


@case
def fill_paths_agree() -> Row:
    """One fill path: a store read, its page and its derive land the same bytes; the tier pins a
    page only once its Region has left the card; alternating Regions refill nothing."""
    with tempfile.TemporaryDirectory() as directory:
        writer = tensorfs.Writer(Path(directory) / "raw.cozytensors")
        source = torch.arange(4 * MIB, dtype=torch.uint8)
        writer.component("net").put("bytes", source)
        checkpoint = writer.commit()
        gpu = varena.Device(0)
        tier = varena.Pages(0, cap=1 << 34)
        stream = torch.cuda.current_stream().cuda_stream
        parts = [("net/bytes#value", 0, source.nbytes)]

        def region(page: str | None, derive: Callable[[torch.Tensor], object] | None = None) -> "varena.Region":
            return gpu.region(source.nbytes, varena.Source(
                tier, "core", 0, source.nbytes, checkpoint=checkpoint, parts=parts, page=page, derive=derive,
            ))

        def same(region: "varena.Region", expected: torch.Tensor) -> None:
            used = region.use(stream)
            assert bool((used.tensor.cpu() == expected).all())
            del used
            gc.collect()
            torch.cuda.synchronize()

        def origins() -> list[str]:
            return [row[3] for row in tier.drain()]

        calls: list[int] = []

        def bake(view: torch.Tensor) -> bool:
            calls.append(torch.cuda.current_stream().cuda_stream)
            assert threading.current_thread() is threading.main_thread()
            view.add_(1)
            return True

        stored, baked = region("stored"), region("baked", bake)
        same(stored, source)
        assert origins() == ["store"]
        assert not tier.pending(), "a resident page is not pinned"
        same(baked, source + 1)
        assert len(calls) == 1 and origins() == ["store"]
        # Both off the card: the stored page pins at idle, the baked one was kept at its bake.
        give_back()
        assert not stored.resident and not baked.resident
        assert tier.pending()
        tier.pin_idle(lambda: False)
        assert not tier.pending() and tier.stats()["pages"] == 2, tier.stats()
        same(stored, source)
        same(baked, source + 1)
        assert origins() == ["baked", "baked"] and len(calls) == 1, "a kept page bakes nothing"
        # Resident, both Regions serve with no fill at all.
        same(stored, source)
        same(baked, source + 1)
        assert not origins()
        # A failed derive reaches its use with its own type, and publishes nothing.
        def fails(view: torch.Tensor) -> bool:
            raise LookupError("derive")

        broken = region(None, fails)
        try:
            broken.use(stream)
        except LookupError:
            pass
        else:
            raise AssertionError("a derive's error reaches its use")
        assert not broken.resident
        tier.forget()
        assert tier.stats()["pages"] == 0
        del stored, baked, broken
        clear(gpu)
        checkpoint.close()
    return {"origins": "store, baked, none"}


@case
def capture_refuses_a_fill() -> Row:
    """A use during capture never runs a derive."""
    gpu = varena.Device(0)
    ran: list[torch.Tensor] = []
    bytes = 2 * MIB
    region = gpu.region(bytes, varena.Source(pages(), "core", 0, bytes, derive=ran.append))
    stream = torch.cuda.current_stream().cuda_stream
    guard = _ext._VarenaAllocatorCapture(0)
    try:
        region.use(stream)
    except RuntimeError as error:
        assert "capture" in str(error), error
    else:
        raise AssertionError("a fill ran during capture")
    finally:
        guard.close()
    assert not ran
    clear(gpu)
    return {"refused": True}


if MODE == "varena":
    warm = torch.empty(64 * MIB, dtype=torch.uint8, device="cuda").fill_(1)
    torch.cuda._sleep(1000)
    warm.add_(1)
    int(warm.sum())
    torch.cuda.synchronize()
    del warm
    torch.cuda.empty_cache()

if MODE == "native":
    def report(name: str, row: Row) -> None:
        print(json.dumps({"case": name, **row}), flush=True)
    report("fast-path", CASES["fast-path"]())
else:
    for name, function in CASES.items():
        if SELECTED in ("all", name):
            print(json.dumps({"case": name, **function()}), flush=True)
            torch.cuda.synchronize()
            row = vt.stats(0)
            print(json.dumps({"after": name, "mapped": row["mapped_bytes"], "pools": row["pools"], "event_handles": row["event_handles"]}), flush=True)
