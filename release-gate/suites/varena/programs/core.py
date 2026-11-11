"""varena's memory-authority rules through its public API (`tensorfs.varena.torch`), one fresh process per run: `python
core.py native fast-path` (stock Torch, the cost comparison) or `python core.py varena <case|all>`. A plain program on the
rental (`release-gate/programs.sh`, xs-174), not a package. The cases that exist only through the Runtime's private
adapter (`tensorfs.varena._runtime`, `_ext`) are findings in README.md, not here."""
import gc
import json
import os
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Literal, cast

import torch

if TYPE_CHECKING:
    import tensorfs.varena.torch as vt

#: The driver's view, read on purpose: this program tests the memory manager itself (no executor's seal here).
mem_get_info = torch.cuda.mem_get_info

MODE, SELECTED = sys.argv[1], sys.argv[2]
os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "1"  # Runtime's setting
if MODE == "varena":
    import tensorfs.varena.torch as vt

    vt.install(0)
torch.cuda.set_device(0)
MIB = 1 << 20
SIZES = (512, 513, 1536, 4096, 65536, MIB, MIB + 17, 8 * MIB)
COST: tuple[Literal["driver_memory_calls", "event_records", "event_queries", "event_waits", "context_joins"], ...] = (
    "driver_memory_calls", "event_records", "event_queries", "event_waits", "context_joins")
type JSON = None | bool | int | float | str | Sequence[JSON] | Mapping[str, JSON]
type Row = Mapping[str, JSON]  # what a case prints
CASES: dict[str, Callable[[], Row]] = {}
SLEEP_S = 1_500_000_000  # torch.cuda._sleep cycles: about a second of real GPU work


def case(function: Callable[[], Row]) -> Callable[[], Row]:
    CASES[function.__name__.replace("_", "-")] = function
    return function


def cap(managed: int) -> None:
    """A development cap leaving this much for our own bytes, the core's slack on top."""
    free, total = mem_get_info(0)
    owned = vt.stats(0)["mapped_bytes"]
    vt.set_limit(0, 1 << 50)  # Any cap makes the core read the driver's reservation.
    reserved = vt.stats(0)["driver_reservation"]
    vt.set_limit(0, managed + max(total - free - owned, 0) + (128 << 20) + reserved)


def give_back() -> None:
    """Everything idle, through the one sequence, as a foreign program's pressure would."""
    vt.set_limit(0, 1)
    vt.set_limit(0, None)


def delta(before: "Mapping[str, object]", after: "Mapping[str, object]") -> dict[str, int]:
    """The change in each of COST's counters: `vt.stats`'s rows are a TypedDict of ints, which mypy reads as objects."""

    def count(stats: "Mapping[str, object]", key: str) -> int:
        value = stats[key]
        if not isinstance(value, int):
            raise TypeError(f"vt.stats {key!r} is {value!r}, not a count")
        return value

    return {key: count(after, key) - count(before, key) for key in COST}


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
    before = vt.stats(0) if MODE == "varena" else None
    samples: list[float] = []
    for _ in range(7):
        began = time.perf_counter_ns()
        pairs(20_000)
        samples.append((time.perf_counter_ns() - began) / (20_000 * len(SIZES)))
    row: dict[str, JSON] = {"ns_per_pair": sorted(samples)[3], "samples": samples}
    if before is not None:
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
