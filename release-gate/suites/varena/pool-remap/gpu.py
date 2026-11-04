"""Replay after a graph pool's pages were released, overwritten and mapped again at the same VA.

One arm per fresh process, on a rental over `cozy machine ssh`; `alias` and `lazy_workspace` are
expected to fault, so never run those on a laptop or through `cozy run`. Each arm prints RESULT.

Every byte comparison is against a replay of the same graph on the same input with no unmap.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import sys
import time

parser = argparse.ArgumentParser()
parser.add_argument("arm")
parser.add_argument("--cycles", type=int, default=24)
parser.add_argument("--dtype", default="float16")
parser.add_argument("--depth", type=int, default=2)
ARGS = parser.parse_args()
if ARGS.arm in ("runtime", "same_skip", "lazy_workspace"):
    os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "1"

import torch  # noqa: E402
import torch.nn.functional as F  # noqa: E402
from tensorfs.varena import torch as vt  # noqa: E402

DEVICE = 0
DTYPE = getattr(torch, ARGS.dtype)
device = torch.device("cuda", DEVICE)
RESULT: dict[str, object] = {"arm": ARGS.arm, "steps": []}
cuda = ctypes.CDLL("libcuda.so.1")
cuda.cuMemsetD8Async.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t, ctypes.c_void_p]
cuda.cuMemcpyDtoH_v2.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t]


def say(step: str, **facts: object) -> None:
    RESULT["steps"].append({"step": step, **facts})
    print(json.dumps({"step": step, **facts}), flush=True)


def finish(code: int = 0) -> None:
    print("RESULT " + json.dumps(RESULT), flush=True)
    os._exit(code)


class Block(torch.nn.Module):
    """A Conv2d, GroupNorm, LayerNorm, attention, Linear and an explicit cuBLAS matmul, with the
    cross-attention shape (154, 2048) x (2048, 1280) that writes its cuBLAS workspace."""

    def __init__(self, channels: int = 320, dim: int = 640, heads: int = 10, rng: bool = False) -> None:
        super().__init__()
        self.dim, self.heads, self.rng = dim, heads, rng
        self.group = torch.nn.GroupNorm(32, channels)
        self.conv = torch.nn.Conv2d(channels, channels, 3, padding=1)
        self.into = torch.nn.Conv2d(channels, dim, 1)
        self.norm = torch.nn.LayerNorm(dim)
        self.query, self.key, self.value = (torch.nn.Linear(dim, dim, bias=False) for _ in range(3))
        self.up, self.down = torch.nn.Linear(dim, 4 * dim), torch.nn.Linear(4 * dim, dim)
        self.weight = torch.nn.Parameter(torch.eye(dim) + 0.01 * torch.randn(dim, dim))
        self.back = torch.nn.Conv2d(dim, channels, 1)
        self.text = torch.nn.Linear(2048, 1280, bias=False)
        self.words = torch.nn.Linear(1280, dim)
        self.context = None

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        b, _, h, w = x.shape
        y = self.into(self.conv(F.silu(self.group(x)))).flatten(2).transpose(1, 2)
        z = self.norm(y)
        split = lambda t: t.reshape(b, h * w, self.heads, self.dim // self.heads).transpose(1, 2)
        a = F.scaled_dot_product_attention(split(self.query(z)), split(self.key(z)), split(self.value(z)))
        y = y + a.transpose(1, 2).reshape(b, h * w, self.dim)
        y = y + self.words(self.text(self.context)).mean(1, keepdim=True)
        y = y + self.down(F.gelu(self.up(self.norm(y))))
        y = torch.matmul(y, self.weight)
        if self.rng:
            y = y + 0.01 * torch.randn_like(y)
        return x + self.back(y.transpose(1, 2).reshape(b, self.dim, h, w))


def model(depth: int = ARGS.depth, rng: bool = False, seed: int = 138) -> torch.nn.Module:
    torch.manual_seed(seed)
    net = torch.nn.Sequential(*[Block(rng=rng) for _ in range(depth)]).to(device, DTYPE).eval()
    context = torch.randn(2, 77, 2048, dtype=torch.float32).to(device, DTYPE)
    for block in net:
        block.context = context
    return net


def sample(cycle: int) -> torch.Tensor:
    """A different input every cycle."""
    return torch.randn(2, 320, 32, 32, generator=torch.Generator().manual_seed(1000 + cycle), dtype=torch.float32).to(device, DTYPE)


def pool_segments(pool) -> list[tuple[int, int]]:
    return [(s["address"], s["total_size"]) for s in torch.cuda.memory_snapshot(mempool_id=pool)
            if tuple(s.get("segment_pool_id", pool)) == tuple(pool)]


def in_pool(pointer: int, ranges) -> bool:
    return any(base <= pointer < base + size for base, size in ranges)


def poison(ranges, stream, value: int = 0xE7) -> None:
    for base, size in ranges:
        code = cuda.cuMemsetD8Async(base, value, size, stream.cuda_stream)
        assert code == 0, f"cuMemsetD8Async {code}"
    stream.synchronize()


def read_head(address: int, count: int = 1 << 16) -> bytes:
    buffer = ctypes.create_string_buffer(count)
    code = cuda.cuMemcpyDtoH_v2(buffer, address, count)
    assert code == 0, f"cuMemcpyDtoH {code}"
    return buffer.raw


def replay(graph, stream) -> None:
    with vt.replay_scope(DEVICE):
        vt.replay(graph, stream=stream)


class Captured:
    """One graph in one pool: a static input outside it, or made in it as Runtime's are."""

    def __init__(self, net, stream, pool=None, reserve=256 << 20, in_pool_static=False, shape=(2, 320, 32, 32)) -> None:
        self.net, self.stream = net, stream
        self.pool = tuple(pool or torch.cuda.graph_pool_handle())
        self.graph = torch.cuda.CUDAGraph()
        if not in_pool_static:
            self.input = torch.empty(shape, dtype=DTYPE, device=device)
            self.input.copy_(sample(0))
        with vt.graph(self.graph, pool=self.pool, stream=stream, reserve_bytes=reserve):
            if in_pool_static:
                self.input = torch.empty(shape, dtype=DTYPE, device=device)
            self.output = net(self.input)
        self.ranges = pool_segments(self.pool)
        self.bytes = sum(size for _, size in self.ranges)

    def run(self, value: torch.Tensor, seed: int | None = None) -> bytes:
        self.input.copy_(value)
        if seed is not None:
            torch.cuda.manual_seed(seed)
        replay(self.graph, self.stream)
        self.stream.synchronize()
        return self.output.cpu().numpy().tobytes()


def squeeze(leave: int = 64 << 20) -> torch.Tensor:
    """Hold the card so only about `leave` bytes stay free: released pages are then the only ones."""
    free, total = torch.cuda.mem_get_info(DEVICE)
    while True:
        try:
            hold = torch.empty(free - leave, dtype=torch.uint8, device=device)
            break
        except torch.OutOfMemoryError:
            leave += 128 << 20
    hold.fill_(0x11)
    torch.cuda.current_stream(DEVICE).synchronize()
    say("squeezed", free_before=free, total=total, held=hold.numel(), free_after=torch.cuda.mem_get_info(DEVICE)[0])
    return hold


def other_data(bytes_: int, stream, pattern: int = 0x5A) -> None:
    """A real tensor in the released pages, written, read back, then gone."""
    other = torch.full((bytes_,), pattern, dtype=torch.uint8, device=device)
    stream.synchronize()
    assert bool((other[:: max(bytes_ // 4096, 1)] == pattern).all())
    del other
    stream.synchronize()


def cycles(item: Captured, stream, count: int, *, seed: bool = False, leave_check: bool = True) -> dict[str, object]:
    identical = 0
    differ: list[int] = []
    head_kept = head_pattern = head_zero = 0
    unmap_ms, remap_ms, first_ms, control_ms = [], [], [], []
    for cycle in range(count):
        value = sample(cycle)
        began = time.perf_counter()
        control = item.run(value, seed=7000 + cycle if seed else None)
        control_ms.append((time.perf_counter() - began) * 1e3)
        old = read_head(item.ranges[0][0])
        began = time.perf_counter()
        released = vt._unmap_pool(item.pool, DEVICE)
        unmap_ms.append((time.perf_counter() - began) * 1e3)
        assert released == item.bytes, (released, item.bytes)
        other_data(item.bytes, stream)
        began = time.perf_counter()
        vt.prepare_pool(item.pool, 0, DEVICE)
        remap_ms.append((time.perf_counter() - began) * 1e3)
        head = read_head(item.ranges[0][0])
        head_kept += head == old
        head_pattern += head == b"\x5a" * len(head)
        head_zero += head == bytes(len(head))
        poison(item.ranges, stream)
        began = time.perf_counter()
        after = item.run(value, seed=7000 + cycle if seed else None)
        first_ms.append((time.perf_counter() - began) * 1e3)
        if after == control:
            identical += 1
        else:
            differ.append(cycle)
    return {
        "cycles": count, "identical": identical, "differ": differ[:8], "pool_bytes": item.bytes,
        "head_same_as_before": head_kept, "head_other_data": head_pattern, "head_zero": head_zero,
        "unmap_ms": rounded(unmap_ms), "remap_ms": rounded(remap_ms),
        "replay_ms_no_unmap": rounded(control_ms), "replay_ms_after_remap": rounded(first_ms),
    }


def rounded(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)
    return {"min": round(ordered[0], 3), "median": round(ordered[len(ordered) // 2], 3), "max": round(ordered[-1], 3)}


def live_blocks(pool) -> dict[str, object]:
    live = [(b["requested_size"], b["size"]) for s in torch.cuda.memory_snapshot(mempool_id=pool)
            if tuple(s.get("segment_pool_id", pool)) == tuple(pool)
            for b in s["blocks"] if b["state"] == "active_allocated"]
    return {"live_blocks": len(live), "live_requested": sorted(r for r, _ in live)[:32]}


def arm_baseline(stream) -> None:
    """Two private pools that cannot both be resident: each cycle's other data is the other graph."""
    item = Captured(model(), stream)
    say("captured", pool_bytes=item.bytes, segments=len(item.ranges), **live_blocks(item.pool))
    hold = squeeze()
    say("cycles", **cycles(item, stream, ARGS.cycles))
    say("stats", **{k: v for k, v in vt.stats(DEVICE).items() if k in ("mapped_bytes", "pools_bytes", "pools", "ooms", "unassigned_bytes")})
    del hold


def arm_two_pools(stream) -> None:
    """A and B each own a pool, the card fits one: A unmapped while B replays, and back."""
    a, b = Captured(model(seed=11), stream), Captured(model(seed=12), stream)
    assert a.bytes == b.bytes
    control_a, control_b = [], []
    for cycle in range(ARGS.cycles):
        control_a.append(a.run(sample(cycle)))
        control_b.append(b.run(sample(cycle + 500)))
    vt._unmap_pool(b.pool, DEVICE)
    hold = squeeze()
    same_a = same_b = 0
    for cycle in range(ARGS.cycles):
        poison(a.ranges, stream)
        same_a += a.run(sample(cycle)) == control_a[cycle]
        assert vt._unmap_pool(a.pool, DEVICE) == a.bytes
        vt.prepare_pool(b.pool, 0, DEVICE)
        poison(b.ranges, stream)
        same_b += b.run(sample(cycle + 500)) == control_b[cycle]
        assert vt._unmap_pool(b.pool, DEVICE) == b.bytes
        vt.prepare_pool(a.pool, 0, DEVICE)
    say("alternated", cycles=ARGS.cycles, a_identical=same_a, b_identical=same_b, pool_bytes=a.bytes,
        free_with_one_mapped=torch.cuda.mem_get_info(DEVICE)[0])
    del hold


def arm_rng(stream) -> None:
    """RNG seed and offset tensors land in the pool by the capture thread's route. Torch refills them each replay."""
    item = Captured(model(rng=True), stream)
    live = live_blocks(item.pool)
    eight = [r for r in live["live_requested"] if r <= 16]
    first, second = item.run(sample(0), seed=5), item.run(sample(0), seed=6)
    say("captured", pool_bytes=item.bytes, small_live_blocks_in_pool=eight, seed_changes_output=first != second,
        same_seed_repeats=item.run(sample(0), seed=5) == first, **live)
    hold = squeeze()
    say("cycles", **cycles(item, stream, ARGS.cycles, seed=True))
    # An eager RNG draw on this stream between replays must not depend on the pool.
    vt._unmap_pool(item.pool, DEVICE)
    drawn = torch.randn(4, device=device).cpu().tolist()
    vt.prepare_pool(item.pool, 0, DEVICE)
    say("eager_rng_while_unmapped", drawn=drawn)
    del hold


class Domain:
    """Runtime's graph domain: control captures, library computes, a third stream replays."""

    def __init__(self) -> None:
        self.library, self.control = torch.cuda.Stream(device=DEVICE), torch.cuda.Stream(device=DEVICE)
        self.current = torch.cuda.Stream(device=DEVICE)
        self.fork, self.join = torch.cuda.Event(), torch.cuda.Event()
        self.fork.record(self.control)
        self.join.record(self.library)
        self.owner = torch.cuda.CUDAGraph()
        with vt.graph(self.owner, stream=self.library, reserve_bytes=0):
            self.join.record(self.library)
        with torch.cuda.stream(self.library):
            for dtype in (torch.bfloat16, torch.float16, torch.float32):
                a = torch.ones(8, 8, dtype=dtype, device=device)
                torch.mm(a, a)
            self.library.synchronize()

    def capture(self, net, pool, shape, reserve) -> "RuntimeGraph":
        with torch.cuda.stream(self.library):
            eager = net(sample(0))  # warm-up with ordinary allocations
            self.library.synchronize()
            del eager
        graph = torch.cuda.CUDAGraph()
        self.control.wait_stream(self.current)
        self.control.wait_stream(self.library)
        statics, outputs = [], []
        with vt.graph(graph, pool=pool, stream=self.control, reserve_bytes=reserve):
            self.fork.record(self.control)
            self.library.wait_event(self.fork)
            with torch.cuda.stream(self.library):
                statics.append(torch.empty(shape, dtype=DTYPE, device=device))
                outputs.append(net(statics[0]))
                self.join.record(self.library)
            self.control.wait_event(self.join)
        self.current.wait_stream(self.control)
        self.current.wait_stream(self.library)
        return RuntimeGraph(graph, statics[0], outputs[0], self, pool)


class RuntimeGraph:
    def __init__(self, graph, static, output, domain, pool) -> None:
        self.graph, self.static, self.output, self.domain, self.pool = graph, static, output, domain, pool

    def run(self, value: torch.Tensor | None) -> bytes:
        stream = self.domain.current
        with torch.cuda.stream(stream):
            if value is not None:
                self.static.copy_(value)
            replay(self.graph, stream)
            out = torch.empty_like(self.output)
            out.copy_(self.output)
            vt.record_pool_stream(self.pool, stream)
            stream.synchronize()
            return out.cpu().numpy().tobytes()


def arm_runtime(stream) -> None:
    """Flag 1, Runtime's streams, one shared pool grown per graph, statics made inside it."""
    domain = Domain()
    pool = tuple(torch.cuda.graph_pool_handle())
    a = domain.capture(model(seed=21), pool, (2, 320, 32, 32), 256 << 20)
    b = domain.capture(model(seed=22), pool, (2, 320, 32, 32), 512 << 20)
    ranges = pool_segments(pool)
    size = sum(s for _, s in ranges)
    say("captured", flag=vt._workspace_cache_enabled(), segments=len(ranges), pool_bytes=size, **live_blocks(pool),
        statics_in_pool=in_pool(a.static.data_ptr(), ranges) and in_pool(b.static.data_ptr(), ranges))
    hold = squeeze()
    same_a = same_b = 0
    differ = []
    for cycle in range(ARGS.cycles):
        va, vb = sample(cycle), sample(cycle + 300)
        ca, cb = a.run(va), b.run(vb)
        assert vt._unmap_pool(pool, DEVICE) == size
        other_data(size, domain.current)
        vt.prepare_pool(pool, 0, DEVICE)
        poison(ranges, domain.current)
        ua, ub = a.run(va), b.run(vb)
        same_a += ua == ca
        same_b += ub == cb
        if ua != ca or ub != cb:
            differ.append(cycle)
    say("cycles", cycles=ARGS.cycles, a_identical=same_a, b_identical=same_b, differ=differ[:8])
    del hold


def arm_same_skip(stream) -> None:
    """`_same` in Runtime skips the copy when the caller's tensor is the static itself."""
    domain = Domain()
    pool = tuple(torch.cuda.graph_pool_handle())
    a = domain.capture(model(seed=31), pool, (2, 320, 32, 32), 256 << 20)
    ranges = pool_segments(pool)
    size = sum(s for _, s in ranges)
    value = sample(9)
    control = a.run(value)
    repeat = a.run(None)  # the static still holds its contents
    assert vt._unmap_pool(pool, DEVICE) == size
    other_data(size, domain.current)
    vt.prepare_pool(pool, 0, DEVICE)
    skipped = a.run(None)
    restored = a.run(value)
    say("same_skip", no_unmap_repeat_identical=repeat == control, skipped_copy_identical=skipped == control,
        recopied_identical=restored == control)


def arm_aux_write(stream) -> None:
    """Eager work on a non-capturing stream during capture, in the capture thread's route."""
    aux = torch.cuda.Stream(device=DEVICE)
    net = model()
    graph = torch.cuda.CUDAGraph()
    pool = tuple(torch.cuda.graph_pool_handle())
    static = torch.empty((2, 320, 32, 32), dtype=DTYPE, device=device)
    static.copy_(sample(0))
    try:
        with vt.graph(graph, pool=pool, stream=stream, reserve_bytes=256 << 20):
            with torch.cuda.stream(aux):
                constant = torch.full((2, 320, 32, 32), 0.5, dtype=DTYPE, device=device)
            output = net(static + constant)
    except RuntimeError as error:
        say("aux_write_refused_in_capture", code=getattr(error, "error_code", None), message=str(error)[:300])
        return
    ranges = pool_segments(pool)
    aux.synchronize()
    written_at_capture = constant.cpu()
    say("captured", constant_in_pool=in_pool(constant.data_ptr(), ranges), constant_filled_eagerly=bool((written_at_capture == 0.5).all()),
        pool_streams=len(ranges))
    def run() -> bytes:
        replay(graph, stream)
        stream.synchronize()
        return output.cpu().numpy().tobytes()
    control = run()
    size = sum(s for _, s in ranges)
    assert vt._unmap_pool(pool, DEVICE) == size
    other_data(size, stream)
    vt.prepare_pool(pool, 0, DEVICE)
    poison(ranges, stream)
    say("replay_after_remap", identical=run() == control, constant_after=constant[0, 0, 0, 0].item())


def arm_alias(stream) -> None:
    """An escaped alias of a pool output, read while the pool is unmapped."""
    item = Captured(model(), stream)
    item.run(sample(0))
    alias = item.output.detach()
    assert vt._unmap_pool(item.pool, DEVICE) == item.bytes
    try:
        got = alias.cpu().numpy().tobytes()
        say("alias_read_unmapped", outcome="returned", bytes=len(got))
    except BaseException as error:  # noqa: BLE001 - this is the measurement
        say("alias_read_unmapped", outcome="error", code=getattr(error, "error_code", None), message=str(error)[:300])
    try:
        alias.add_(1.0)
        stream.synchronize()
        say("alias_kernel_unmapped", outcome="returned")
    except BaseException as error:  # noqa: BLE001 - this is the measurement
        say("alias_kernel_unmapped", outcome="error", code=getattr(error, "error_code", None), message=str(error)[:300])
    finish(3)


def arm_lazy_workspace(stream) -> None:
    """Flag 1 with no primed workspace: it is first made inside capture, so in the pool."""
    stream = torch.cuda.Stream(device=DEVICE)  # no eager cuBLAS has run on this one
    net = torch.nn.Linear(2048, 1280, bias=False).to(device, DTYPE).eval()
    static = torch.randn(2, 77, 2048, dtype=DTYPE, device=device)
    graph = torch.cuda.CUDAGraph()
    pool = tuple(torch.cuda.graph_pool_handle())
    torch.cuda.set_stream(stream)
    with vt.graph(graph, pool=pool, stream=stream, reserve_bytes=64 << 20):
        output = net(static)
    ranges = pool_segments(pool)
    want = torch.backends.cuda.cublas_workspace_size()
    in_it = [b["requested_size"] for s in torch.cuda.memory_snapshot(mempool_id=pool) for b in s["blocks"] if b["requested_size"] == want]
    replay(graph, stream)
    stream.synchronize()
    control = output.cpu().numpy().tobytes()
    say("captured", workspace_in_pool=bool(in_it), pool_bytes=sum(s for _, s in ranges))
    assert vt._unmap_pool(pool, DEVICE) > 0
    try:
        eager = net(static)
        stream.synchronize()
        say("eager_cublas_while_unmapped", outcome="pass", identical=eager.cpu().numpy().tobytes() == control)
    except BaseException as error:  # noqa: BLE001 - this is the measurement
        say("eager_cublas_while_unmapped", outcome="error", code=getattr(error, "error_code", None), message=str(error)[:300])
        finish(3)


def arm_inflight(stream) -> None:
    """Unmap refuses (0) while a recorded replay is in flight, and works once it completes."""
    item = Captured(model(depth=8), stream)
    item.input.copy_(sample(0))
    refused = []
    for _ in range(10):
        replay(item.graph, stream)
        refused.append(vt._unmap_pool(item.pool, DEVICE))
        if refused[-1]:
            vt.prepare_pool(item.pool, 0, DEVICE)
        stream.synchronize()
    say("in_flight", returned=refused, refused_while_pending=sum(r == 0 for r in refused))
    assert vt._unmap_pool(item.pool, DEVICE) == item.bytes
    vt.prepare_pool(item.pool, 0, DEVICE)
    item.input.copy_(sample(0))
    replay(item.graph, stream)
    stream.synchronize()


def arm_refusals(stream) -> None:
    """Misuse is a Python error, no GPU fault; a reset while unmapped frees the VA once."""
    item = Captured(model(), stream)
    item.run(sample(0))
    mapped = vt.stats(DEVICE)["mapped_bytes"]
    assert vt._unmap_pool(item.pool, DEVICE) == item.bytes
    assert vt._unmap_pool(item.pool, DEVICE) == 0
    after = vt.stats(DEVICE)
    assert mapped - after["mapped_bytes"] == item.bytes and after["pools_bytes"] == 0, after
    errors = {}
    try:
        replay(item.graph, stream)
        errors["replay"] = None
    except RuntimeError as error:
        errors["replay"] = str(error)[:200]
    try:
        graph = torch.cuda.CUDAGraph()
        graph.capture_begin(pool=item.pool)
        errors["capture"] = None
    except RuntimeError as error:
        errors["capture"] = str(error)[:200]
    say("refused", **errors)
    addresses = item.ranges
    del item.output
    item.graph.reset()
    trimmed = vt.trim(DEVICE)
    late = vt.stats(DEVICE)
    say("reset_while_unmapped", trimmed=trimmed, pools=late["pools"], pools_bytes=late["pools_bytes"],
        mapped=late["mapped_bytes"], unassigned=late["unassigned_bytes"])
    again = Captured(model(), stream)
    say("recaptured_after", reused_address=again.ranges[0][0] == addresses[0][0], output_bytes=len(again.run(sample(1))))


def arm_timing(stream) -> None:
    """Remap time per GiB against reset, capture, instantiate of the same graph."""
    net = model()
    sweep = []
    for size in (256 << 20, 1 << 30, 4 << 30, 8 << 30):
        item = Captured(net, stream, reserve=size)
        reference = item.run(sample(0))
        unmap, remap, first = [], [], []
        same = 0
        for _ in range(8):
            began = time.perf_counter()
            assert vt._unmap_pool(item.pool, DEVICE) == item.bytes
            unmap.append((time.perf_counter() - began) * 1e3)
            began = time.perf_counter()
            vt.prepare_pool(item.pool, 0, DEVICE)
            remap.append((time.perf_counter() - began) * 1e3)
            began = time.perf_counter()
            same += item.run(sample(0)) == reference
            first.append((time.perf_counter() - began) * 1e3)
        sweep.append({"pool_bytes": item.bytes, "unmap_ms": rounded(unmap), "remap_ms": rounded(remap),
                      "first_replay_ms": rounded(first), "identical": same,
                      "remap_ms_per_gib": round(sorted(remap)[len(remap) // 2] / (item.bytes / (1 << 30)), 3)})
        del item.output, item.input
        item.graph.reset()
        stream.synchronize()
        vt.trim(DEVICE)
    say("sweep", sizes=sweep)
    for depth in (2, 12, 40):
        net = model(depth=depth)
        trials = []
        for trial in range(4):
            began = time.perf_counter()
            item = Captured(net, stream, reserve=1 << 30)
            captured = time.perf_counter()
            item.run(sample(trial))
            replayed = time.perf_counter()
            remaps = []
            for _ in range(4):
                assert vt._unmap_pool(item.pool, DEVICE) == item.bytes
                start = time.perf_counter()
                vt.prepare_pool(item.pool, 0, DEVICE)
                remaps.append((time.perf_counter() - start) * 1e3)
            trials.append({"capture_ms": round((captured - began) * 1e3, 2), "first_replay_ms": round((replayed - captured) * 1e3, 2),
                           "remap_ms": rounded(remaps), "pool_bytes": item.bytes})
            del item.output, item.input
            item.graph.reset()
            stream.synchronize()
            vt.trim(DEVICE)
        say("recapture", depth=depth, trials=trials[1:])


vt.install(DEVICE)
torch.cuda.set_device(DEVICE)
say("start", pid=os.getpid(), tensorfs=vt.__file__, torch=torch.__version__, gpu=torch.cuda.get_device_name(DEVICE),
    dtype=ARGS.dtype, flag=vt._workspace_cache_enabled())
capture_stream = torch.cuda.Stream(device=DEVICE)
with torch.inference_mode(), torch.cuda.stream(capture_stream):
    warm = model()
    for _ in range(2):
        warm(sample(0))
    capture_stream.synchronize()
    del warm
    globals()["arm_" + ARGS.arm](capture_stream)
finish(0)
