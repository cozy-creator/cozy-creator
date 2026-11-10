"""One fault per process: `cells.py <cell> <role>`. run.py owns kills, hang watching and judging.

Each role prints JSON steps and ends with a `verdict` step whose outcome is one of:
correct (bytes match), typed (a clean refusal), corrupt (wrong bytes, no error),
device_fault (the GPU saw it), wrong_refusal (refused with room to spare) or n/a.
"""
from __future__ import annotations

import ctypes
import faulthandler
import gc
import json
import os
import random
import select
import signal
import sys
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, cast

CELL, ROLE = sys.argv[1], sys.argv[2]
HERE = Path(__file__).resolve().parent
DIR = Path(os.environ["FAULT_DIR"])
FLAG_1 = {"workspace_unmapped", "workspace_reuse", "lazy_workspace"}
if ROLE != "probe":
    os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "1" if CELL in FLAG_1 else "0"
faulthandler.enable()
faulthandler.dump_traceback_later(30, repeat=True)

import torch  # noqa: E402
import torch.nn.functional as F  # noqa: E402
from tensorfs import _ext
from tensorfs.varena import _runtime as varena  # noqa: E402
from tensorfs.varena import api  # noqa: E402
from tensorfs.varena import torch as vt  # noqa: E402
from tensorfs.varena.torch import PoolId

if TYPE_CHECKING:
    from tensorfs.varena._offload import Offload

sys.path[:0] = [str(HERE.parent.parent / "native-tensor-export"), str(HERE.parent.parent / "ledger")]
from provider import Provider  # noqa: E402
import table as ledger  # noqa: E402

DEVICE, MIB, HALF = 0, 1 << 20, torch.float16
device = torch.device("cuda", DEVICE)
cuda = ctypes.CDLL("libcuda.so.1")
cuda.cuMemsetD8Async.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t, ctypes.c_void_p]
cuda.cuMemsetD8_v2.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t]
cuda.cuMemcpyDtoH_v2.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t]


def say(step: str, **facts: object) -> None:
    print(json.dumps({"step": step, **facts}), flush=True)


def verdict(outcome: str, **facts: object) -> None:
    say("verdict", outcome=outcome, **facts)


def classify(error: BaseException) -> str:
    text = str(error)
    if isinstance(error, torch.AcceleratorError) or any(
        mark in text for mark in ("CUDA error", "CUDA device fault", "poisoned")
    ):
        return "device_fault"
    if isinstance(error, (api.Error, torch.OutOfMemoryError)):
        return "typed"
    if any(mark in text for mark in ("invalid residency operation", "after fork", "install tensorfs.varena.torch",
                                     "Cannot re-initialize CUDA in forked subprocess", "escaped its owner")):
        return "typed"
    return "untyped"


class Block(torch.nn.Module):
    """A Conv2d, norms, attention, Linear, a cuBLAS matmul and the cross-attention shape
    (154, 2048) x (2048, 1280) that writes its cuBLAS workspace."""

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
        self.context: torch.Tensor | None = None

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        b, _, h, w = x.shape
        y = self.into(self.conv(F.silu(self.group(x)))).flatten(2).transpose(1, 2)
        z = self.norm(y)
        split: Callable[[torch.Tensor], torch.Tensor] = lambda t: t.reshape(b, h * w, self.heads, self.dim // self.heads).transpose(1, 2)
        a = F.scaled_dot_product_attention(split(self.query(z)), split(self.key(z)), split(self.value(z)))
        y = y + a.transpose(1, 2).reshape(b, h * w, self.dim)
        assert self.context is not None
        y = y + self.words(self.text(self.context)).mean(1, keepdim=True)
        y = y + self.down(F.gelu(self.up(self.norm(y))))
        y = torch.matmul(y, self.weight)
        if self.rng:
            y = y + 0.01 * torch.randn_like(y)
        out: torch.Tensor = x + self.back(y.transpose(1, 2).reshape(b, self.dim, h, w))
        return out


def model(depth: int = 2, seed: int = 138, rng: bool = False) -> torch.nn.Module:
    torch.manual_seed(seed)
    net = torch.nn.Sequential(*[Block(rng=rng) for _ in range(depth)]).to(device, HALF).eval()
    context = torch.randn(2, 77, 2048, dtype=torch.float32).to(device, HALF)
    for block in net:
        block.context = context
    return net


SHAPE = (2, 320, 32, 32)


def sample(cycle: int) -> torch.Tensor:
    generator = torch.Generator().manual_seed(1000 + cycle)
    return torch.randn(*SHAPE, generator=generator, dtype=torch.float32).to(device, HALF)


def raw(tensor: torch.Tensor) -> bytes:
    return tensor.cpu().numpy().tobytes()


def replay(graph: torch.cuda.CUDAGraph, stream: torch.cuda.Stream) -> None:
    with vt.replay_scope(DEVICE):
        vt.replay(graph, stream=stream)


def give_back() -> int:
    """Everything idle back to the driver: the core's one sequence, as foreign pressure asks it."""
    torch.cuda.empty_cache()
    taken = vt.set_limit(DEVICE, 1)
    vt.set_limit(DEVICE, None)
    return taken


def unmap_pool(pool: PoolId, held: list[torch.Tensor | None], stream: torch.cuda.Stream) -> int:
    """Declare a pool's tensors dead between uses, end this use, and let the core take its pages."""
    vt.pool_scratch(pool, held, DEVICE)
    vt.record_pool_stream(pool, stream)
    stream.synchronize()
    before = vt.stats(DEVICE)["mapped_bytes"]
    deadline = time.monotonic() + 2  # The end-of-use fence is sealed, then seen done, by a later poll.
    while True:
        give_back()
        taken = before - vt.stats(DEVICE)["mapped_bytes"]
        if taken or time.monotonic() > deadline:
            return taken
        time.sleep(0.001)


def pool_ranges(pool: PoolId) -> list[tuple[int, int]]:
    return [(s["address"], s["total_size"]) for s in torch.cuda.memory_snapshot(mempool_id=pool)]


def poison(ranges: list[tuple[int, int]], stream: torch.cuda.Stream, value: int = 0xE7) -> None:
    for base, size in ranges:
        assert cuda.cuMemsetD8Async(base, value, size, stream.cuda_stream) == 0
    stream.synchronize()


class Graph:
    """One graph in a private pool; its static input outside it, or inside as Runtime's are."""

    def __init__(self, net: torch.nn.Module, stream: torch.cuda.Stream, reserve: int = 256 * MIB,
                 static_in_pool: bool = False) -> None:
        self.net, self.stream = net, stream
        self.pool = cast(PoolId, tuple(torch.cuda.graph_pool_handle()))  # a (int, int) handle
        self.graph = torch.cuda.CUDAGraph()
        if not static_in_pool:
            self.input = torch.empty(SHAPE, dtype=HALF, device=device)
            self.input.copy_(sample(0))
        with vt.graph(self.graph, pool=self.pool, stream=stream, reserve_bytes=reserve):
            if static_in_pool:
                self.input = torch.empty(SHAPE, dtype=HALF, device=device)
            self.output: torch.Tensor | None = net(self.input)
        self.ranges = pool_ranges(self.pool)
        self.bytes = sum(size for _, size in self.ranges)

    def run(self, value: torch.Tensor | None, seed: int | None = None) -> bytes:
        if value is not None:
            self.input.copy_(value)
        if seed is not None:
            torch.cuda.manual_seed(seed)
        replay(self.graph, self.stream)
        self.stream.synchronize()
        assert self.output is not None
        return raw(self.output)


def rows() -> list[dict[str, int]]:
    """This machine's ledger, as its peers read it."""
    return [row for path in ledger.tables() for row in ledger.rows(path)[1]]


def setup() -> tuple[torch.cuda.Stream, None]:
    """A fresh process's device, a stream, warm kernels. Install joins the machine's ledger, as an
    executor does."""
    vt.install(DEVICE)
    torch.cuda.set_device(DEVICE)
    stream = torch.cuda.Stream(device=DEVICE)
    torch.cuda.set_stream(stream)
    with torch.inference_mode():
        warm = model(depth=1)
        warm(sample(0))
        stream.synchronize()
        del warm
    return stream, None


def squeeze(leave: int = 64 * MIB) -> torch.Tensor:
    """Hold the card so released pages are the only free ones."""
    free, _ = torch.cuda.mem_get_info(DEVICE)
    while True:
        try:
            hold = torch.empty(free - leave, dtype=torch.uint8, device=device)
            break
        except torch.OutOfMemoryError:
            leave += 128 * MIB
    hold.fill_(0x11)
    torch.cuda.current_stream(DEVICE).synchronize()
    say("squeezed", held=hold.numel(), free=torch.cuda.mem_get_info(DEVICE)[0])
    return hold


def sleep_forever() -> None:
    while True:
        time.sleep(1)


# ---- Peers that keep working across someone else's fault ----

def survivor() -> None:
    stream, _ = setup()
    net = model(seed=7)
    graph = Graph(net, stream)
    controls = [graph.run(sample(i)) for i in range(8)]
    say("ready", pid=os.getpid())
    beats = bad = 0
    while not (DIR / "stop").exists():
        i = beats % 8
        bad += graph.run(sample(i)) != controls[i]
        churn = torch.empty(64 * MIB, dtype=torch.uint8, device=device)
        churn.fill_(beats % 251)
        bad += int(churn[-1]) != beats % 251
        del churn
        beats += 1
        if beats % 10 == 0:
            say("beat", beats=beats, bad=bad)
        time.sleep(0.005)
    verdict("correct" if bad == 0 else "corrupt", beats=beats, bad=bad, ledger_pids=[r["pid"] for r in rows()])


def probe() -> None:
    """The GPU and any ledger still work, in a fresh process: eager and replay agree."""
    stream, _ = setup()
    net = model(seed=9)
    value = sample(3)
    eager = raw(net(value))
    graph = Graph(net, stream)
    same = graph.run(value) == eager
    verdict("correct" if same else "corrupt", replay_matches_eager=same)


# ---- Executors killed by run.py at a named point ----

def kill_capture() -> None:
    stream, _ = setup()
    net = model(seed=8)
    static = sample(1)
    graph = torch.cuda.CUDAGraph()
    with vt.graph(graph, pool=torch.cuda.graph_pool_handle(), stream=stream, reserve_bytes=256 * MIB):
        output = net(static)
        say("kill_me", at="inside capture")
        sleep_forever()


def kill_replay() -> None:
    stream, _ = setup()
    graph = Graph(model(depth=8, seed=8), stream)
    graph.run(sample(1))
    for n in range(100000):
        replay(graph.graph, stream)
        if n == 40:
            say("kill_me", at="replays queued", queued=n)


def kill_fill() -> None:
    stream, _ = setup()
    gpu = api.open(DEVICE)
    region = gpu.region(bytes(4096 * MIB))
    say("kill_me", at="about to fill 4 GiB")
    use = gpu.use([region], stream.cuda_stream)
    say("fill_submitted", resident=gpu.stats()["weights_resident"])
    sleep_forever()


def kill_fill_gated() -> None:
    stream, _ = setup()
    gpu = api.open(DEVICE)
    fixture = Provider(os.environ["FAULT_PROVIDER"]).create(gpu, 64 * MIB, mode="ordering")
    use = gpu.use([fixture.region], stream.cuda_stream)
    say("kill_me", at="fill pending behind a closed gate", pending=fixture.source_pending())
    sleep_forever()


def donor() -> None:
    stream, _ = setup()
    idle = int(os.environ["FAULT_IDLE"])
    live = torch.full((8 * MIB,), 23, dtype=torch.uint8, device=device)
    pieces = [torch.empty(2 * MIB, dtype=torch.uint8, device=device) for _ in range(idle // (2 * MIB))]
    for piece in pieces[:: 64]:
        piece.fill_(19)
    stream.synchronize()
    del pieces
    gc.collect()
    deadline = time.monotonic() + 30
    while True:
        mine = next(row for row in rows() if row["pid"] == os.getpid())
        if mine["idle"] >= idle * 9 // 10:  # The live tensor keeps its segment's pages mapped.
            break
        assert time.monotonic() < deadline, mine
        time.sleep(0.01)
    say("ready", reclaimable=mine["idle"], pid=os.getpid())
    sleep_forever()


def asker() -> None:
    setup()
    if os.environ.get("FAULT_CAP"):
        vt.set_limit(DEVICE, torch.cuda.mem_get_info(DEVICE)[1] + vt.stats(DEVICE)["driver_reservation"])
    say("ready", pid=os.getpid())
    while not (DIR / "go").exists():
        time.sleep(0.005)
    free, _ = torch.cuda.mem_get_info(DEVICE)
    want = free + int(os.environ["FAULT_IDLE"]) * 3 // 4
    say("asking", free=free, want=want)
    began = time.monotonic()
    big = torch.empty(want, dtype=torch.uint8, device=device)
    big.fill_(0x33)
    torch.cuda.current_stream(DEVICE).synchronize()
    good = int(big[0]) == int(big[-1]) == int(big[want // 2]) == 0x33
    verdict("correct" if good else "corrupt", waited_s=round(time.monotonic() - began, 3), bytes=want)


# ---- A Source failing, and fills under eviction ----

def source_failing(mode: str) -> None:
    stream, _ = setup()
    gpu = api.open(DEVICE)
    net = model(seed=10)
    graph = Graph(net, stream)
    control = graph.run(sample(2))
    fixture = Provider(os.environ["FAULT_PROVIDER"]).create(gpu, 8 * MIB, mode=mode)
    try:
        use = gpu.use([fixture.region], stream.cuda_stream)
        verdict("corrupt", detail="a failed Source produced a Use")
        return
    except api.SourceError as error:
        say("source_refused", type=type(error).__name__, message=str(error)[:200])
    fixture.region.close()
    fixture.gate_set(1)
    deadline = time.monotonic() + 20
    while api.retirement_stats()["queued"] and time.monotonic() < deadline:
        time.sleep(0.01)
    stats = api.retirement_stats()
    same = graph.run(sample(2)) == control
    payload = bytes(range(256)) * (MIB // 256)
    region = gpu.region(payload)
    with gpu.use([region], stream.cuda_stream) as use:
        data = use.tensor(dtype="uint8", shape=(len(payload),))
        filled = raw(data) == payload
        del data
    good = same and filled and not stats["quarantined"] and not stats["queued"]
    verdict("typed" if good else "corrupt", replay_correct=same, later_fill_correct=filled, retirement=stats,
            counts=fixture.counts())


def source_partial() -> None:
    source_failing("partial")


def source_prepare_error() -> None:
    source_failing("prepare_error")


def evict_fill_gated() -> None:
    """A fill pending behind a gate while everything that evicts runs, then the bytes."""
    stream, _ = setup()
    gpu = api.open(DEVICE)
    nbytes = 64 * MIB
    # Warm first: CUDA's lazy kernel load can wait on the whole context, gated producer included.
    warm = torch.empty(2 * MIB, dtype=torch.uint8, device=device)
    warm.fill_(1)
    bool((warm == 1).all())
    stream.synchronize()
    del warm
    fixture = Provider(os.environ["FAULT_PROVIDER"]).create(gpu, nbytes, mode="ordering")
    use = gpu.use([fixture.region], stream.cuda_stream)
    say("fill_pending", pending=fixture.source_pending())
    use.close()
    trims = [give_back() for _ in range(10)]
    say("trimmed", trims=trims, resident=gpu.stats()["weights_resident"], pending=fixture.source_pending())
    vt.set_limit(DEVICE, 16 * MIB)
    say("limited", resident=gpu.stats()["weights_resident"], pending=fixture.source_pending())
    vt.set_limit(DEVICE, None)
    side = torch.cuda.Stream(device=DEVICE)  # `stream` itself waits behind the gated producer
    with torch.cuda.stream(side):
        pressure = torch.empty(1024 * MIB, dtype=torch.uint8, device=device)
        pressure.fill_(1)
        side.synchronize()
        del pressure
    fixture.gate_set(1)
    with gpu.use([fixture.region], stream.cuda_stream) as again:
        again.wait_ready()
        data = again.tensor(dtype="uint8", shape=(nbytes,))
        good = bool((data == 73).all())
        del data
    verdict("correct" if good else "corrupt", counts=fixture.counts())


def evict_fill_stress() -> None:
    """Regions filling on demand under a cap that fits three, with a thread evicting throughout."""
    stream, _ = setup()
    gpu = api.open(DEVICE)
    size, count = 256 * MIB, 8
    payloads = [torch.randint(0, 256, (MIB,), dtype=torch.uint8, generator=torch.Generator().manual_seed(i))
                .repeat(size // MIB) for i in range(count)]
    regions = [gpu.region(api.Source.from_tensor(p)) for p in payloads]
    vt.set_limit(DEVICE, 1024 * MIB)
    stop = threading.Event()
    errors: list[str] = []

    def evict() -> None:
        pressure_stream = torch.cuda.Stream(device=DEVICE)
        with torch.inference_mode(), torch.cuda.stream(pressure_stream):
            while not stop.is_set():
                try:
                    block = torch.empty(128 * MIB, dtype=torch.uint8, device=device)
                    block.fill_(1)
                    pressure_stream.synchronize()
                    del block
                    give_back()
                except BaseException as error:  # noqa: BLE001 - the measurement
                    errors.append(f"{classify(error)} {type(error).__name__}: {str(error)[:120]}")
                time.sleep(0.002)

    thread = threading.Thread(target=evict)
    thread.start()
    end, reads, wrong, refused = time.monotonic() + 25, 0, 0, 0
    pick = random.Random(138)
    try:
        while time.monotonic() < end:
            i = pick.randrange(count)
            try:
                with gpu.use([regions[i]], stream.cuda_stream) as use:
                    data = use.tensor(dtype="uint8", shape=(size,))
                    host = data[:: 4093].cpu()
                    wrong += not torch.equal(host, payloads[i][:: 4093])
                    del data
                reads += 1
            except (api.Exhausted, api.Busy, torch.OutOfMemoryError):
                refused += 1
            if (reads + refused) % 10 == 0:
                say("beat", reads=reads, refused=refused, wrong=wrong)
    finally:
        stop.set()
        thread.join()
    vt.set_limit(DEVICE, None)
    faults = [e for e in errors if not e.startswith("typed")]
    verdict("corrupt" if wrong else "device_fault" if faults else "correct", reads=reads, refused=refused,
            wrong=wrong, evictor_errors=len(errors), faults=faults[:3], resident=gpu.stats()["weights_resident"])


# ---- Capture refusals ----

def capture_oom_pool() -> None:
    stream, _ = setup()
    net = model(seed=11)
    value = sample(4)
    eager = raw(net(value))
    try:
        Graph(net, stream, reserve=2 * MIB)
        verdict("corrupt", detail="a 2 MiB pool held the whole capture")
        return
    except torch.OutOfMemoryError as error:
        say("refused", type=type(error).__name__, message=str(error)[:200])
    stream.synchronize()
    same = Graph(net, stream).run(value) == eager
    verdict("typed" if same else "corrupt", retry_replay_matches_eager=same)


def capture_oom_physical() -> None:
    stream, _ = setup()
    net = model(seed=11)
    value = sample(4)
    eager = raw(net(value))
    hold = squeeze(16 * MIB)
    try:
        Graph(net, stream, reserve=1024 * MIB)
        verdict("wrong_refusal", detail="1 GiB fit a card with 16 MiB free")
        return
    except torch.OutOfMemoryError as error:
        say("refused", type=type(error).__name__, message=str(error)[:200])
    del hold
    stream.synchronize()
    same = Graph(net, stream).run(value) == eager
    verdict("typed" if same else "corrupt", retry_replay_matches_eager=same)


def create_oom(reclaimable: bool) -> None:
    """A segment whose 5th handle the driver refuses (`fail_create_after`): what it mapped first is
    reported and retired as one, so the shim sees no stranger. Refused once, the allocation
    reclaims, retries and holds its bytes; refused for good, it fails cleanly and the next one
    works. 17e3a8d23 poisoned here."""
    stream, _ = setup()
    give_back()
    if reclaimable:
        spare = torch.empty(64 * MIB, dtype=torch.uint8, device=device)
        del spare  # completed and idle: the first step of the reclaim order takes it
        stream.synchronize()
    _ext._varena_fail_create_after(DEVICE, 5, 1 if reclaimable else 1 << 62)
    refused: torch.OutOfMemoryError | None
    try:
        big = torch.empty(256 * MIB, dtype=torch.uint8, device=device)  # 16 handles of 16 MiB
    except torch.OutOfMemoryError as error:
        refused = error
    else:
        refused = None
        big.fill_(0x5A)
        stream.synchronize()
        held = bool((big == 0x5A).all())
    _ext._varena_fail_create_after(DEVICE, 0, 0)
    after = torch.empty(256 * MIB, dtype=torch.uint8, device=device)
    after.fill_(0x3C)
    stream.synchronize()
    stats = vt.stats(DEVICE)  # a poisoned owner raises here
    say("allocated", refused=refused is not None, physical_ooms=stats["driver_physical_ooms"],
        mapped=stats["observed_mapped"], retired=stats["observed_retired"])
    if stats["driver_physical_ooms"] < 1:
        verdict("n/a", detail="no handle was refused")
    elif reclaimable:
        verdict("correct" if refused is None and held else "wrong_refusal", refused=str(refused)[:200])
    else:
        verdict("typed" if refused is not None else "n/a", refused=str(refused)[:200])


def create_oom_retry() -> None:
    create_oom(reclaimable=True)


def create_oom_refused() -> None:
    create_oom(reclaimable=False)


def fit_pool_rng_late() -> None:
    """The first RNG tensor of a capture, after its stream freed temporaries in the pool."""
    stream, _ = setup()
    net = model(seed=12, rng=True)
    try:
        graph = Graph(net, stream)
    except torch.OutOfMemoryError as error:
        verdict("wrong_refusal", message=str(error)[:200], pool_bytes=256 * MIB)
        return
    first, second = graph.run(sample(5), seed=5), graph.run(sample(5), seed=6)
    again = graph.run(sample(5), seed=5)
    verdict("correct" if first == again and first != second else "corrupt", seed_repeats=first == again)


# ---- Raw Torch, fork, foreign memory ----

def raw_graph(prepared: bool = False) -> None:
    """Stock torch.cuda.graph with none of Varena's hooks, then work that must not be stuck."""
    stream, _ = setup()
    net = model(seed=13)
    value = sample(6)
    eager = raw(net(value))
    static = value.clone()
    stream.synchronize()
    graph: torch.cuda.CUDAGraph | None
    graph = torch.cuda.CUDAGraph()
    output: torch.Tensor | None = None
    pool = cast(PoolId, tuple(torch.cuda.graph_pool_handle()))  # a (int, int) handle
    if prepared:
        vt.prepare_pool(pool, 256 * MIB, DEVICE)
    try:
        with torch.cuda.graph(graph, pool=pool):  # type: ignore[arg-type]  # a plain (int, int), not torch's _POOL_HANDLE NewType
            output = net(static)
        say("raw_captured")
    except BaseException as error:  # noqa: BLE001 - the measurement
        say("raw_capture_refused", kind=classify(error), type=type(error).__name__, message=str(error)[:300])
        graph = None
    same: bool | None = None
    trimmed: bool | None = None
    if graph is not None:
        assert output is not None
        graph.replay()
        stream.synchronize()
        same = raw(output) == eager
        say("raw_replayed", identical=same)
        give_back()
        pressure = torch.empty(512 * MIB, dtype=torch.uint8, device=device)
        pressure.fill_(1)
        stream.synchronize()
        del pressure
        give_back()
        graph.replay()
        stream.synchronize()
        trimmed = raw(output) == eager
        say("raw_replayed_after_trim", identical=trimmed)
    after = torch.empty(64 * MIB, dtype=torch.uint8, device=device)
    after.fill_(7)
    stream.synchronize()
    say("eager_after", value=int(after[0]))
    del after
    follow = Graph(net, stream).run(value) == eager
    good = follow and same is not False and trimmed is not False
    verdict(("correct" if graph is not None else "typed") if good else "corrupt", raw_replay=same,
            raw_replay_after_trim=trimmed, vt_graph_after=follow)


def raw_graph_prepared() -> None:
    """The same with a mapped pool, so the capture itself succeeds and only its hooks are missing."""
    raw_graph(prepared=True)


def forked_child(write: int, tensor: torch.Tensor, graph: Graph) -> None:
    results: dict[str, str] = {}
    holder = [tensor]
    gpu_ops: dict[str, Callable[[], object]] = {
        "free": holder.clear,
        "open": lambda: api.open(DEVICE),
        "alloc": lambda: torch.empty(8, device=device),
        "stats": lambda: vt.stats(DEVICE),
        "trim": give_back,
        "pool": lambda: vt.prepare_pool(graph.pool, 0, DEVICE),
        "replay": lambda: vt.replay(graph.graph, stream=graph.stream),
    }
    for name, op in gpu_ops.items():
        try:
            op()
            results[name] = "returned"
        except BaseException as error:  # noqa: BLE001 - the measurement
            results[name] = f"{classify(error)}: {str(error)[:90]}"
    os.write(write, json.dumps(results).encode())
    os._exit(0)


def fork() -> None:
    stream, _ = setup()
    graph = Graph(model(seed=14), stream)
    control = graph.run(sample(7))
    held = torch.ones(16, device=device)
    run = threading.Event()
    run.set()

    def busy() -> None:
        with torch.inference_mode():
            while run.is_set():
                t = torch.empty(2 * MIB, dtype=torch.uint8, device=device)
                del t

    thread = threading.Thread(target=busy)
    thread.start()
    hung = 0
    seen: dict[str, int] = {}
    messages: dict[str, str] = {}
    for n in range(20):
        read, write = os.pipe()
        pid = os.fork()
        if pid == 0:
            os.close(read)
            forked_child(write, held, graph)
        os.close(write)
        ready, _, _ = select.select([read], [], [], 15)
        if not ready:
            hung += 1
            os.kill(pid, signal.SIGKILL)
        else:
            answered: dict[str, str] = json.loads(os.read(read, 65536).decode())
            for key, value in answered.items():
                label = f"{key}={value.split(':')[0]}"
                seen[label] = seen.get(label, 0) + 1
                messages.setdefault(label, value)
        os.waitpid(pid, 0)
        os.close(read)
        say("forked", n=n, hung=hung)
    run.clear()
    thread.join()
    same = graph.run(sample(7)) == control
    returned = [k for k in seen if k.endswith("=returned") and not k.startswith(("free", "stats", "trim"))]
    outcome = "corrupt" if not same or returned else "device_fault" if hung else "typed"
    verdict(outcome, forks=20, hung=hung, child_results=seen, messages=messages, parent_correct=same)


def worker() -> None:
    """Keeps replaying and allocating while another program takes the memory."""
    stream, _ = setup()
    graph = Graph(model(seed=15), stream)
    controls = [graph.run(sample(i)) for i in range(4)]
    say("ready")
    beats = bad = refused = 0
    last_ok = False
    while not (DIR / "stop").exists():
        bad += graph.run(sample(beats % 4)) != controls[beats % 4]
        try:
            block = torch.empty(512 * MIB, dtype=torch.uint8, device=device)
            block.fill_(5)
            stream.synchronize()
            bad += int(block[-1]) != 5
            del block
            stream.synchronize()
            give_back()  # back to the driver, where the other program can take it
            last_ok = True
        except torch.OutOfMemoryError:
            refused += 1
            last_ok = False
            say("refused", beats=beats, bad=bad, refused=refused)
        beats += 1
        if beats % 50 == 0:
            say("beat", beats=beats, bad=bad, refused=refused)
    verdict("correct" if bad == 0 and last_ok else "corrupt", beats=beats, bad=bad, refused=refused, recovered=last_ok)


def foreign() -> None:
    """A plain Torch program, no Varena: takes all it can, holds, leaves."""
    torch.cuda.set_device(DEVICE)
    chunks: list[torch.Tensor] = []
    grabbed = 0
    while True:
        try:
            chunk = torch.empty(256 * MIB, dtype=torch.uint8, device=device)
            chunk.fill_(9)
            chunks.append(chunk)
            grabbed += 256 * MIB
        except torch.OutOfMemoryError:
            break
    say("grabbed", grabbed=grabbed, free=torch.cuda.mem_get_info(DEVICE)[0])
    while not (DIR / "foreign-leave").exists():  # keeps taking what the other side gives back
        try:
            chunks.append(torch.empty(64 * MIB, dtype=torch.uint8, device=device).fill_(9))
            grabbed += 64 * MIB
        except torch.OutOfMemoryError:
            torch.cuda.empty_cache()
        time.sleep(0.01)
    verdict("correct", grabbed=grabbed)


# ---- Unmaps under running kernels, and pages scrambled ----

def unmap_under_kernel() -> None:
    stream, _ = setup()
    gpu = api.open(DEVICE)
    graph = Graph(model(depth=12, seed=16), stream, reserve=512 * MIB)
    value = sample(8)
    reference = graph.run(value)

    def attack_while_running() -> dict[str, object]:
        unmaps, attempts = 0, 0
        while not stream.query():
            attempts += 1
            unmaps += give_back()
            vt.set_limit(DEVICE, 64 * MIB)
            vt.set_limit(DEVICE, None)
        return {"attempts": attempts, "unmapped_bytes": unmaps}

    vt.pool_scratch(graph.pool, [graph.input, graph.output], DEVICE)
    graph.input.copy_(value)
    for _ in range(30):
        replay(graph.graph, stream)
    facts = attack_while_running()
    stream.synchronize()
    assert graph.output is not None
    same = raw(graph.output) == reference
    say("under_replay", identical=same, **facts)

    a = torch.ones(2048, 2048, device=device) / 2048
    b = a
    for _ in range(40):
        b = b @ a
    expected = raw(b)
    a = torch.ones(2048, 2048, device=device) / 2048
    c = a
    for _ in range(40):
        c = c @ a
    del a
    facts = attack_while_running()
    stream.synchronize()
    eager_same = raw(c) == expected
    say("freed_under_eager", identical=eager_same, **facts)
    vt.prepare_pool(graph.pool, 0, DEVICE)

    out = torch.empty_like(graph.output)
    graph.input.copy_(value)
    for _ in range(30):
        replay(graph.graph, stream)
    out.copy_(graph.output)
    vt.record_pool_stream(graph.pool, stream)
    graph.graph.reset()
    del graph.output
    facts = attack_while_running()
    stream.synchronize()
    reset_same = raw(out) == reference
    say("reset_under_replay", identical=reset_same, **facts)
    verdict("correct" if same and eager_same and reset_same else "corrupt")


def remap_scrambled() -> None:
    """After unmap the pool's pages come from holes left by a shuffled set of live tensors."""
    stream, _ = setup()
    graph = Graph(model(seed=17), stream)
    hold = squeeze()
    same = refused = 0
    shuffle = random.Random(138)
    cycles = 24
    pieces: list[torch.Tensor] | None
    for cycle in range(cycles):
        value = sample(cycle)
        control = graph.run(value)
        assert unmap_pool(graph.pool, [graph.input, graph.output], stream) >= graph.bytes
        pieces = [torch.empty(2 * MIB, dtype=torch.uint8, device=device) for _ in range(graph.bytes // (2 * MIB))]
        for piece in pieces:
            piece.fill_(0x5A)
        stream.synchronize()
        shuffle.shuffle(pieces)
        pieces = pieces[: len(pieces) // 8]
        gc.collect()
        give_back()
        try:
            vt.prepare_pool(graph.pool, 0, DEVICE)
        except torch.OutOfMemoryError:
            refused += 1
            pieces = None
            gc.collect()
            give_back()
            vt.prepare_pool(graph.pool, 0, DEVICE)
        pieces = None
        poison(graph.ranges, stream)
        same += graph.run(value) == control
        if cycle % 4 == 0:
            say("beat", cycle=cycle, same=same, refused=refused)
    del hold
    verdict("correct" if same == cycles else "corrupt", cycles=cycles, identical=same, remap_refused_then_retried=refused)


def same_skip() -> None:
    """A pool's contents don't survive between uses, so an owner copies its inputs in every time:
    one that skips the copy reads whatever the remap found. That is why Runtime stopped."""
    stream, _ = setup()
    graph = Graph(model(seed=18), stream, static_in_pool=True)
    control = graph.run(sample(9))
    repeat = graph.run(None)
    assert unmap_pool(graph.pool, [graph.input, graph.output], stream) >= graph.bytes
    other = torch.full((graph.bytes,), 0x5A, dtype=torch.uint8, device=device)
    stream.synchronize()
    del other
    vt.prepare_pool(graph.pool, 0, DEVICE)
    skipped = graph.run(None)
    recopied = graph.run(sample(9)) == control
    verdict("correct" if recopied else "corrupt", repeat_identical=repeat == control,
            skipped_identical=skipped == control, recopied_identical=recopied)


def aux_write() -> None:
    """Eager work on a non-capturing stream during capture lands in the pool and is never replayed."""
    stream, _ = setup()
    aux = torch.cuda.Stream(device=DEVICE)
    net = model(seed=19)
    graph = torch.cuda.CUDAGraph()
    pool = cast(PoolId, tuple(torch.cuda.graph_pool_handle()))  # a (int, int) handle
    static = sample(0)
    with vt.graph(graph, pool=pool, stream=stream, reserve_bytes=256 * MIB):
        with torch.cuda.stream(aux):
            constant = torch.full(SHAPE, 0.5, dtype=HALF, device=device)
        output = net(static + constant)
    aux.synchronize()
    replay(graph, stream)
    stream.synchronize()
    control = raw(output)
    ranges = pool_ranges(pool)
    assert unmap_pool(pool, [output], stream) >= sum(size for _, size in ranges)
    vt.prepare_pool(pool, 0, DEVICE)
    poison(ranges, stream)
    replay(graph, stream)
    stream.synchronize()
    same = raw(output) == control
    verdict("correct" if same else "corrupt", identical=same)


def alias_unmapped() -> None:
    """An escaped alias of a pool output refuses the declaration, so no kernel can touch it unmapped."""
    stream, _ = setup()
    graph = Graph(model(seed=20), stream)
    graph.run(sample(0))
    assert graph.output is not None
    alias = graph.output.detach()
    unmap_pool(graph.pool, [graph.input, graph.output], stream)
    alias.add_(1.0)
    stream.synchronize()
    verdict("corrupt", detail="the add returned on an unmapped alias")


def lazy_workspace() -> None:
    """Flag 1, no primed workspace: it is made inside capture, in the pool, then unmapped."""
    setup()
    fresh = torch.cuda.Stream(device=DEVICE)
    torch.cuda.set_stream(fresh)
    net = torch.nn.Linear(2048, 1280, bias=False).to(device, HALF).eval()
    static = torch.randn(2, 77, 2048, dtype=HALF, device=device)
    graph = torch.cuda.CUDAGraph()
    pool = cast(PoolId, tuple(torch.cuda.graph_pool_handle()))  # a (int, int) handle
    with vt.graph(graph, pool=pool, stream=fresh, reserve_bytes=64 * MIB):
        output = net(static)
    replay(graph, fresh)
    fresh.synchronize()
    control = raw(output)
    assert unmap_pool(pool, [static, output], fresh) > 0
    eager = net(static)
    fresh.synchronize()
    verdict("correct" if raw(eager) == control else "corrupt")


class NoStream(torch.nn.Module):
    """A model whose library launches a kernel with no stream argument, as Sage does."""

    def __init__(self) -> None:
        super().__init__()
        self.first, self.second = torch.nn.Linear(256, 256), torch.nn.Linear(256, 256)

    def forward(self, value: torch.Tensor) -> torch.Tensor:
        middle = self.first(value)
        if not middle.is_cuda:  # The reference, computed before any CUDA.
            reference: torch.Tensor = self.second(middle.add(1.0))
            return reference
        with torch.cuda.stream(torch.cuda.default_stream(DEVICE)):
            middle = middle.add(1.0)
        out: torch.Tensor = self.second(middle)
        return out


def no_stream_adopted(graphs: str) -> tuple[NoStream, torch.Tensor, torch.Tensor, Offload]:
    torch.manual_seed(138)
    net = NoStream().eval()
    value = torch.randn(8, 256)
    reference = net(value)
    handle: Offload = vt.offload(net, device=DEVICE, graphs=graphs)
    handle.owner.current()
    return net, value.to(device), reference, handle


def no_stream_eager() -> None:
    """On the execution stream: program order, with no fault and no silent wrong output."""
    net, value, reference, _ = no_stream_adopted("eager")
    target = torch.empty(64 * MIB, dtype=torch.uint8, device=device)
    torch.cuda._sleep(2_000_000_000)  # So work queued on the stream runs well after this returns.
    target.fill_(1)
    with torch.cuda.stream(torch.cuda.default_stream(DEVICE)):
        target.add_(6)
    seen = torch.unique(target.clone()).tolist()
    same = all(torch.allclose(net(value).cpu(), reference, atol=1e-5) for _ in range(4))
    verdict("correct" if seen == [7] and same else "corrupt", seen=seen, identical=same)


def no_stream_capture() -> None:
    """Under capture: it can't be in the graph, so the capture falls back to eager, output correct."""
    net, value, reference, handle = no_stream_adopted("auto")
    same = all(torch.allclose(net(value).cpu(), reference, atol=1e-5) for _ in range(12))
    assert handle.graphs is not None
    counts = handle.graphs.counts
    say("counts", captures=counts["captures"], replays=counts["replays"], declines=str(counts["declines"])[:200])
    # CUDA's own refusal: a check of one replay against eager would only see what changes its output.
    fell_back = not counts["replays"] and any("capturing blocking stream" in why for why in counts["declines"])
    verdict("correct" if same and fell_back else "corrupt", identical=same, fell_back=fell_back)


def no_stream_graph() -> None:
    """vt.graph with no stream given captures on a blocking one, so CUDA refuses a stray launch."""
    vt.install(DEVICE)
    value = torch.ones(16, device=device)
    refused = None
    try:
        with vt.graph(torch.cuda.CUDAGraph()), torch.cuda.stream(torch.cuda.default_stream(DEVICE)):
            value.add(1.0)
    except torch.AcceleratorError as error:
        refused = str(error)
    verdict("correct" if refused and "capturing blocking stream" in refused else "corrupt", refused=str(refused)[:120])


def workspace_graphs(stream: torch.cuda.Stream) -> tuple[Graph, Graph, int | None, int]:
    """Two graphs on one stream at flag 1, sharing Torch's cached workspace."""
    a, b = Graph(model(seed=21), stream), Graph(model(seed=22), stream)
    a.run(sample(0))
    want = torch.backends.cuda.cublas_workspace_size()
    found = [block["address"] for segment in torch.cuda.memory_snapshot() for block in segment["blocks"]
             if block["requested_size"] == want and block["state"] == "active_allocated"]
    return a, b, found[0] if len(found) == 1 else None, want


def workspace_unmapped() -> None:
    """A sibling's reset frees the workspace, trim unmaps it, the other graph replays."""
    stream, _ = setup()
    a, b, address, want = workspace_graphs(stream)
    reference = b.run(sample(1))
    a.graph.reset()
    a.output = None
    stream.synchronize()
    give_back()
    gone = not any(s["address"] <= (address or 0) < s["address"] + s["total_size"] for s in torch.cuda.memory_snapshot())
    say("trimmed", workspace=address, unmapped=gone)
    same = b.run(sample(1)) == reference
    verdict("correct" if same else "corrupt", identical=same, workspace_unmapped=gone)


def workspace_reuse() -> None:
    """A sibling's reset frees the workspace, a new tensor takes its address, the other graph replays."""
    stream, _ = setup()
    a, b, address, want = workspace_graphs(stream)
    reference = b.run(sample(1))
    a.graph.reset()
    a.output = None
    stream.synchronize()
    held: list[torch.Tensor] = []
    found: torch.Tensor | None = None
    for _ in range(64):
        tensor = torch.empty(want, dtype=torch.uint8, device=device)
        if tensor.data_ptr() == address:
            found = tensor
            break
        held.append(tensor)
    if found is None:
        verdict("correct", detail="the workspace's address was not reused")
        return
    found.fill_(0xA5)
    stream.synchronize()
    same = b.run(sample(1)) == reference
    changed = int((found != 0xA5).sum())
    verdict("corrupt" if changed or not same else "correct", overwritten_bytes=changed, output_identical=same)


def read(address: int, bytes: int) -> bytes:
    """What the device holds at an address, whoever owns it now."""
    held = ctypes.create_string_buffer(bytes)
    assert cuda.cuMemcpyDtoH_v2(held, address, bytes) == 0
    return held.raw


def tool_unmap() -> None:
    """The gate's own tool: `vt.checks` takes a pool before its use and scrambles it, so an owner
    that skips its input copy is caught, and the differential cells can be believed."""
    stream, _ = setup()
    graph = Graph(model(seed=21), stream, static_in_pool=True)
    control = graph.run(sample(9))
    assert graph.run(None) == control, "a kept input repeats"
    vt.pool_scratch(graph.pool, [graph.input, graph.output], DEVICE)
    vt.record_pool_stream(graph.pool, stream)
    stream.synchronize()
    vt.checks(unmap_idle=True, perturb=True)
    vt.prepare_pool(graph.pool, 0, DEVICE)
    skipped = graph.run(None)
    vt.record_pool_stream(graph.pool, stream)
    vt.prepare_pool(graph.pool, 0, DEVICE)
    recopied = graph.run(sample(9))
    vt.record_pool_stream(graph.pool, stream)
    vt.checks()
    counts = vt.check_counts()
    gone = counts["uses"] == 2 and counts["unmapped_bytes"] >= 2 * graph.bytes
    good = skipped != control and recopied == control and gone
    verdict("correct" if good else "corrupt", skipped_identical=skipped == control,
            recopied_identical=recopied == control, **counts)


def tool_perturb() -> None:
    """Freed blocks read as the pattern, in their stream's order, and so does the next block given out,
    wherever the allocator puts it (a card with room to spare puts it elsewhere: g34, g42)."""
    stream, _ = setup()
    vt.checks(perturb=True)
    block = torch.full((4 * MIB,), 0x11, dtype=torch.uint8, device=device)
    address = block.data_ptr()
    del block
    stream.synchronize()
    freed = read(address, 4 * MIB)
    again = torch.empty(4 * MIB, dtype=torch.uint8, device=device)
    stream.synchronize()
    given = bytes(again.cpu().numpy())
    vt.checks()
    pattern = b"\xa5" * (4 * MIB)
    good = freed == pattern and given == pattern
    verdict("correct" if good else "corrupt", freed_stale=sum(b != 0xA5 for b in freed),
            given_stale=sum(b != 0xA5 for b in given), reused_at_address=again.data_ptr() == address)


def overrun(guards: bool) -> tuple[int, int, torch.Tensor]:
    """A write just past the first of two large blocks; what that left in the second."""
    stream, _ = setup()
    vt.checks(guards=guards)
    first = torch.zeros(8 * MIB, dtype=torch.uint8, device=device)
    second = torch.zeros(8 * MIB, dtype=torch.uint8, device=device)
    stream.synchronize()
    say("blocks", gap=second.data_ptr() - first.data_ptr() - 8 * MIB)
    code = cuda.cuMemsetD8_v2(first.data_ptr() + 8 * MIB, 0xE7, 4096)
    stream.synchronize()
    return code, int((second != 0).sum()), first


def guard_control() -> None:
    """With no guard an overrun lands on its neighbour and nothing says so: why guards exist."""
    code, changed, _ = overrun(False)
    verdict("correct" if code == 0 and changed else "n/a", memset=code, neighbour_bytes_changed=changed)


def tool_guard() -> None:
    """With guards the same overrun faults, here and in a real model's differential."""
    try:
        code, changed, _ = overrun(True)
    except torch.AcceleratorError as error:
        code, changed = getattr(error, "error_code", 999), 0
    verdict("correct" if code and not changed else "corrupt", memset=code, neighbour_bytes_changed=changed)
    sys.stdout.flush()
    os._exit(0)  # Its context is faulted: nothing past this can leave cleanly.


if __name__ == "__main__":
    try:
        with torch.inference_mode():
            globals()[CELL if ROLE == "main" else ROLE]()
    except BaseException as error:  # noqa: BLE001 - every outcome is a verdict
        code = getattr(error, "error_code", None)
        verdict(classify(error), type=type(error).__name__, code=code, message=str(error)[:400])
        raise SystemExit(0) from error
