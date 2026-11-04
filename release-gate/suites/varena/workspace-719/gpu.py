"""Does a sibling reset unmapping Torch's cached cuBLAS workspace cause CUDA 719? No model.

One arm per fresh process; arm A and the Runtime control deliberately fault. Run these on a rental
over `cozy machine ssh`, never on a laptop or through `cozy run`.

  A   flag 1, one stream: reset graph A, trim, confirm the workspace is unmapped, replay graph B
  B   flag 0, same steps
  C   flag 1, Runtime's control and library streams: reset A, trim, replay B
  C2  C, then also reset the library-stream owner graph: positive control that B uses the workspace
  D   flag 1, one stream: reset A, no trim, fill a tensor at the workspace's address, replay B
  T   flag 1, no fault: which matmul shapes and dtypes write the workspace at all

  E   A's steps, then map only the workspace's VA again: the fault must go away if it is the cause

A suffix x (Ax, Bx, Cx, C2x, Dx, Ex) adds up0's cross-attention Linear to graph B: (154, 2048) x (2048, 1280).
"""
from __future__ import annotations

import ctypes
import json
import os
import sys

ARM = sys.argv[1]
os.environ["TORCH_CUBLAS_WORKSPACE_CACHE"] = "0" if ARM[0] == "B" else "1"

import torch  # noqa: E402
import torch.nn.functional as F  # noqa: E402
from tensorfs.varena import torch as vt  # noqa: E402

DEVICE = 0
HALF = torch.float16
RESULT: dict[str, object] = {"arm": ARM, "steps": []}


def say(step: str, **facts: object) -> None:
    RESULT["steps"].append({"step": step, **facts})
    print(json.dumps({"step": step, **facts}), flush=True)


def finish(code: int) -> None:
    print("RESULT " + json.dumps(RESULT), flush=True)
    os._exit(code)


class Block(torch.nn.Module):
    """The operations and shapes the 27056 trace records for SDXL's UNet: Linear on (2, N, dim)
    and (dim, dim), LayerNorm, GroupNorm, Conv2d and flash attention."""

    def __init__(self, dim: int, heads: int, cross: bool = False) -> None:
        super().__init__()
        self.dim, self.heads = dim, heads
        self.context = None
        if cross:
            self.text_key = torch.nn.Linear(2048, dim, bias=False)
            self.text_value = torch.nn.Linear(2048, dim, bias=False)
        self.resnet_norm = torch.nn.GroupNorm(32, dim)
        self.resnet_conv = torch.nn.Conv2d(dim, dim, 3, padding=1)
        self.norm = torch.nn.LayerNorm(dim)
        self.key = torch.nn.Linear(dim, dim, bias=False)
        self.query = torch.nn.Linear(dim, dim, bias=False)
        self.value = torch.nn.Linear(dim, dim, bias=False)
        self.out = torch.nn.Linear(dim, dim)
        self.up = torch.nn.Linear(dim, 4 * dim)
        self.down = torch.nn.Linear(4 * dim, dim)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        b, c, h, w = x.shape
        x = x + self.resnet_conv(F.silu(self.resnet_norm(x)))
        y = x.flatten(2).transpose(1, 2)
        z = self.norm(y)
        head = self.dim // self.heads
        split = lambda t: t.reshape(b, h * w, self.heads, head).transpose(1, 2)
        a = F.scaled_dot_product_attention(split(self.query(z)), split(self.key(z)), split(self.value(z)))
        y = y + self.out(a.transpose(1, 2).reshape(b, h * w, self.dim))
        if self.context is not None:
            words = lambda t: t.reshape(b, -1, self.heads, head).transpose(1, 2)
            a = F.scaled_dot_product_attention(split(self.query(self.norm(y))), words(self.text_key(self.context)),
                                               words(self.text_value(self.context)))
            y = y + self.out(a.transpose(1, 2).reshape(b, h * w, self.dim))
        y = y + self.down(F.gelu(self.up(self.norm(y))))
        return y.transpose(1, 2).reshape(b, c, h, w)


def stats() -> dict[str, int]:
    return vt.stats(DEVICE)


def workspace() -> dict[str, int] | None:
    """Its block, by Torch's chosen workspace size: the requested size only it allocates."""
    want = torch.backends.cuda.cublas_workspace_size()
    found = []
    for segment in torch.cuda.memory_snapshot():
        for block in segment["blocks"]:
            if block["requested_size"] == want and block["state"] != "inactive":
                found.append({"address": block["address"], "size": block["size"],
                              "segment": segment["address"], "segment_size": segment["total_size"],
                              "pool": list(segment.get("segment_pool_id", ()))})
    say("workspace_blocks", want=want, found=found)
    return found[0] if len(found) == 1 else None


def segments() -> list[list[int]]:
    return sorted([s["address"], s["total_size"]] for s in torch.cuda.memory_snapshot())


def replay(graph: torch.cuda.CUDAGraph, stream: torch.cuda.Stream, out: torch.Tensor) -> bytes:
    with vt.replay_scope(DEVICE):
        vt.replay(graph, stream=stream)
    stream.synchronize()
    return out.cpu().numpy().tobytes()


def fault_or_pass(graph, stream, out, expected, unmapped):
    """Replay with the mapping change in place; report a device error or a byte comparison."""
    try:
        got = replay(graph, stream, out)
    except BaseException as error:  # noqa: BLE001 - this is the measurement
        say("replay_after", outcome="fault", error_code=getattr(error, "error_code", None),
            type=type(error).__name__, message=str(error)[:3000], unmapped=unmapped)
        finish(3)
    say("replay_after", outcome="pass", identical=got == expected, unmapped=unmapped)
    finish(0)


def models(device):
    torch.manual_seed(719)
    a = Block(640, 10).to(device, HALF).eval()
    b = Block(1280, 20, cross=ARM.endswith("x")).to(device, HALF).eval()
    if ARM.endswith("x"):
        b.context = torch.randn(2, 77, 2048, dtype=HALF, device=device)
    input_a = torch.randn(2, 640, 64, 64, dtype=HALF, device=device)
    input_b = torch.randn(2, 1280, 32, 32, dtype=HALF, device=device)
    return a, b, input_a, input_b


def one_stream() -> None:
    stream = torch.cuda.Stream(device=DEVICE)
    device = torch.device("cuda", DEVICE)
    with torch.inference_mode(), torch.cuda.stream(stream):
        a, b, input_a, input_b = models(device)
        eager_a, eager_b = a(input_a), b(input_b)
        eager = eager_b.cpu().numpy().tobytes()
        stream.synchronize()
        before = workspace()
        graph_a, graph_b = torch.cuda.CUDAGraph(), torch.cuda.CUDAGraph()
        pool_a, pool_b = torch.cuda.graph_pool_handle(), torch.cuda.graph_pool_handle()
        with vt.graph(graph_a, pool=pool_a, stream=stream, reserve_bytes=256 << 20):
            out_a = a(input_a)
        with vt.graph(graph_b, pool=pool_b, stream=stream, reserve_bytes=256 << 20):
            out_b = b(input_b)
        replay(graph_a, stream, out_a)
        reference = replay(graph_b, stream, out_b)
        say("captured", flag=vt._workspace_cache_enabled(), replay_matches_eager=reference == eager,
            replay_repeats=replay(graph_b, stream, out_b) == reference, stats=stats())
        kept = segments()
        mapped = stats()["mapped_bytes"]
        graph_a.reset()
        del out_a
        stream.synchronize()
        if ARM[0] == "D":
            del eager_a, eager_b
            found = before and allocate_at(before["address"], stream, device)
            if found is None:
                say("workspace_not_reused", before=before)
                finish(4)
            found.fill_(0xA5)
            stream.synchronize()
            size = found.numel()
            intact = int((found == 0xA5).sum())
            got = replay(graph_b, stream, out_b)
            after = found.cpu()
            changed = (after != 0xA5).nonzero().flatten()
            say("pattern_after_replay", address=found.data_ptr(), size=size, before_replay_intact=intact,
                changed_bytes=changed.numel(), first_changed=changed[:1].tolist(),
                last_changed=changed[-1:].tolist(), output_identical=got == reference)
            finish(0)
        released = vt.trim(DEVICE)
        now = segments()
        gone = [s for s in kept if s not in now]
        say("trimmed", released=released, mapped_before=mapped, mapped_after=stats()["mapped_bytes"],
            unmapped_segments=gone, workspace_segment_gone=bool(before) and [before["segment"], before["segment_size"]] in gone)
        if ARM[0] == "E":
            found = allocate_at(before["address"], stream, device)
            found.fill_(0xA5)
            stream.synchronize()
            got = replay(graph_b, stream, out_b)
            changed = (found.cpu() != 0xA5).nonzero().flatten() + found.data_ptr() - before["address"]
            say("workspace_va_mapped_again", tensor=found.data_ptr(), workspace=before["address"], replay="pass",
                identical=got == reference, changed_bytes=changed.numel(),
                first_changed_in_workspace=changed[:1].tolist(), last_changed_in_workspace=changed[-1:].tolist())
            finish(0)
        fault_or_pass(graph_b, stream, out_b, reference, gone)


def touch() -> None:
    """Fill the live workspace, run one operation, count the bytes it changed."""
    cuda = ctypes.CDLL("libcuda.so.1")
    cuda.cuMemsetD8Async.argtypes = [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t, ctypes.c_void_p]
    cuda.cuMemcpyDtoH_v2.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t]
    stream = torch.cuda.Stream(device=DEVICE)
    device = torch.device("cuda", DEVICE)

    def changed(found: dict[str, int], size: int) -> int:
        buffer = (ctypes.c_ubyte * size)()
        assert cuda.cuMemcpyDtoH_v2(buffer, found["address"], size) == 0
        return size - bytes(buffer).count(0xA5)

    with torch.inference_mode(), torch.cuda.stream(stream):
        for dtype in (HALF, torch.bfloat16, torch.float32):
            m = torch.ones(8, 8, dtype=dtype, device=device)
            torch.mm(m, m)
            torch.addmm(torch.ones(8, dtype=dtype, device=device).diag(), m, m)
        stream.synchronize()
        found = workspace()
        size = torch.backends.cuda.cublas_workspace_size()
        assert cuda.cuMemsetD8Async(found["address"], 0xA5, size, stream.cuda_stream) == 0
        stream.synchronize()
        assert changed(found, size) == 0
        assert cuda.cuMemsetD8Async(found["address"] + 4096, 0, 16, stream.cuda_stream) == 0
        stream.synchronize()
        say("probe_control", detected=changed(found, size), wrote=16)
        touched, cases = [], 0
        grid = [(m, k, n) for m in (154, 2048, 8192, 32768) for k in (64, 320, 640, 768, 1280, 2048, 5120, 10240)
                for n in (640, 1280, 2048, 10240)]
        for dtype in (HALF, torch.bfloat16, torch.float32):
            for m, k, n in grid:
                a, b = torch.ones(m, k, dtype=dtype, device=device), torch.ones(k, n, dtype=dtype, device=device)
                bias = torch.ones(m, n, dtype=dtype, device=device)
                for name, run in (("mm", lambda: torch.mm(a, b)), ("addmm", lambda: torch.addmm(bias, a, b))):
                    assert cuda.cuMemsetD8Async(found["address"], 0xA5, size, stream.cuda_stream) == 0
                    stream.synchronize()
                    run()
                    stream.synchronize()
                    cases += 1
                    count = changed(found, size)
                    if count:
                        touched.append([name, str(dtype), m, k, n, count])
                del a, b, bias
        say("touch_sweep", cases=cases, touching=len(touched), first=touched[:40])
        # The block both graphs use, eagerly and replayed.
        a, b, input_a, input_b = models(device)
        assert cuda.cuMemsetD8Async(found["address"], 0xA5, size, stream.cuda_stream) == 0
        stream.synchronize()
        b(input_b)
        a(input_a)
        stream.synchronize()
        say("touch_block_eager", changed=changed(found, size))
    finish(0)


def allocate_at(address: int, stream, device) -> torch.Tensor | None:
    """Take blocks of the workspace's class until one covers its first byte."""
    held = []
    size = torch.backends.cuda.cublas_workspace_size()
    for _ in range(64):
        tensor = torch.empty(size, dtype=torch.uint8, device=device)
        if tensor.data_ptr() <= address < tensor.data_ptr() + size:
            return tensor
        held.append(tensor)
    say("not_reused", wanted=address, got=[t.data_ptr() for t in held][:8], segments=segments()[:12])
    return None


def runtime_streams() -> None:
    """What `graphs.domain.Domain` and `graphs.capture` do: control captures, library computes."""
    device = torch.device("cuda", DEVICE)
    library, control = torch.cuda.Stream(device=DEVICE), torch.cuda.Stream(device=DEVICE)
    current = torch.cuda.Stream(device=DEVICE)
    fork, join = torch.cuda.Event(), torch.cuda.Event()
    fork.record(control)
    join.record(library)
    owner = torch.cuda.CUDAGraph()
    with vt.graph(owner, stream=library, reserve_bytes=0):
        join.record(library)
    with torch.inference_mode(), torch.cuda.stream(library):
        # Domain.prime_blas: the library stream's workspace exists outside any capture.
        for dtype in (torch.bfloat16, torch.float16, torch.float32):
            m = torch.ones(8, 8, dtype=dtype, device=device)
            torch.mm(m, m)
        library.synchronize()
        before = workspace()
        a, b, input_a, input_b = models(device)
        a(input_a)
        eager = b(input_b).cpu().numpy().tobytes()
        library.synchronize()

        def capture(graph, pool, step):
            control.wait_stream(current)
            control.wait_stream(library)
            with vt.graph(graph, pool=pool, stream=control, reserve_bytes=256 << 20):
                fork.record(control)
                library.wait_event(fork)
                with torch.cuda.stream(library):
                    output = step()
                    join.record(library)
                control.wait_event(join)
            current.wait_stream(control)
            current.wait_stream(library)
            return output

        graph_a, graph_b = torch.cuda.CUDAGraph(), torch.cuda.CUDAGraph()
        pool_a, pool_b = torch.cuda.graph_pool_handle(), torch.cuda.graph_pool_handle()
        out_a = capture(graph_a, pool_a, lambda: a(input_a))
        out_b = capture(graph_b, pool_b, lambda: b(input_b))
        reference = replay(graph_b, current, out_b)
        say("captured", flag=vt._workspace_cache_enabled(), replay_matches_eager=reference == eager,
            replay_repeats=replay(graph_b, current, out_b) == reference, stats=stats())
        kept = segments()
        mapped = stats()["mapped_bytes"]
        graph_a.reset()
        del out_a
        current.synchronize()
        control.synchronize()
        library.synchronize()
        if ARM.startswith("C2"):
            owner.reset()  # Runtime does this only at domain teardown, after every graph.
        released = vt.trim(DEVICE)
        now = segments()
        gone = [s for s in kept if s not in now]
        after = workspace()
        say("trimmed", released=released, mapped_before=mapped, mapped_after=stats()["mapped_bytes"],
            unmapped_segments=gone, workspace_before=before, workspace_after=after,
            workspace_segment_gone=bool(before) and [before["segment"], before["segment_size"]] in gone)
        fault_or_pass(graph_b, current, out_b, reference, gone)


vt.install(DEVICE)
torch.cuda.set_device(DEVICE)
say("start", pid=os.getpid(), tensorfs=vt.__file__, torch=torch.__version__, git=torch.version.git_version,
    gpu=torch.cuda.get_device_name(DEVICE), cublas_workspace=torch.backends.cuda.cublas_workspace_size(),
    cublaslt_workspace=torch.backends.cuda.cublaslt_workspace_size())
{"A": one_stream, "B": one_stream, "D": one_stream, "E": one_stream, "T": touch}.get(ARM[0], runtime_streams)()
