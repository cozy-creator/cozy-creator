"""One node class in a plain step loop: `STEPS + 1` steps, and the values are checked. One variant per
run, so a fault ends only its executor and names its variant. It was the native conditional body
(IF -> WHILE -> body) of `denoise_loop`; Runtime 0.43 has no loop programs, so each step now runs as
written, and the cell keeps each node class's values."""

import json
import subprocess

import msgspec
import torch
import torch.nn.functional as F
import triton
import triton.language as tl
from cozy_runtime.author import App, Context
from cuda.bindings import driver

app = App()
STEPS = 3  # the loop runs STEPS + 1 steps
MIX = ("memset", "memcpy", "cublas", "cudnn", "sdpa", "triton")
VARIANTS = ("scalar", "memset", "memset32", "memcpy", "reduce", "parallel", "triton", "triton_pdl",
            "cublas", "cudnn", "sdpa", "mix", "labels", "legacy")
#: two labels, each run twice
LABELS = ("a", "b", "a", "b")


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    variant: str = "mix"
    seed: int = 331


class Report(msgspec.Struct):
    passed: bool
    detail: str


@triton.jit
def add_one(pointer, count, BLOCK: tl.constexpr):
    offsets = tl.program_id(0) * BLOCK + tl.arange(0, BLOCK)
    mask = offsets < count
    tl.store(pointer + offsets, tl.load(pointer + offsets, mask=mask) + 1.0, mask=mask)


from collections.abc import Callable

#: A step's op, and a variant's check of its values after the loop.
type Op = Callable[[], object]
type Check = tuple[str, Callable[[], bool]]
# What a counting variant's tensor holds when its check fails: each distinct value and how many hold it.
SEEN: dict[str, torch.Tensor] = {}
# Each launch of theirs: its stream, and whether that stream was capturing then.
LAUNCHES: dict[str, set[tuple[str, bool]]] = {}


def launched(name: str, stream: "torch.cuda.Stream") -> None:
    LAUNCHES.setdefault(name, set()).add((hex(stream.cuda_stream), torch.cuda.is_current_stream_capturing()))


def part(name: str, device: torch.device, ops: list[Op], checks: list[Check]) -> None:  # noqa: C901 - one table of small cases
    if name in ("scalar", "labels"):
        return
    if name in ("memset", "memset32"):
        scratch = torch.full((768,), 0xAB, dtype=torch.uint8, device=device)

        def memset(wide: bool = name == "memset32") -> None:
            stream = driver.CUstream(torch.cuda.current_stream().cuda_stream)
            pointer = driver.CUdeviceptr(scratch.data_ptr())
            result = (driver.cuMemsetD32Async(pointer, 0x5A5A5A5A, 768 // 4, stream) if wide
                      else driver.cuMemsetD8Async(pointer, 0x5A, 768, stream))
            assert int(result[0]) == 0, result

        ops.append(memset)
        checks.append((name, lambda: bool((scratch == 0x5A).all())))
    elif name == "memcpy":
        a = torch.randn(8192, device=device)
        b = torch.zeros_like(a)
        ops.append(lambda: b.copy_(a))
        checks.append((name, lambda: torch.equal(a, b)))
    elif name == "cublas":
        cases: list[tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]] = []
        for m, inner, n in ((1, 3072, 18432), (8, 3072, 3072), (1024, 3072, 3072)):
            h = torch.randn(m, inner, dtype=torch.bfloat16, device=device)
            w = torch.randn(inner, n, dtype=torch.bfloat16, device=device) / inner ** 0.5
            cases.append((h, w, torch.empty(m, n, dtype=torch.bfloat16, device=device), torch.mm(h, w)))
        ops.append(lambda: [torch.mm(h, w, out=o) for h, w, o, _ in cases])
        checks.append((name, lambda: all(torch.equal(o, r) for _, _, o, r in cases)))
    elif name == "cudnn":
        x = torch.randn(1, 320, 64, 64, dtype=torch.bfloat16, device=device)
        w = torch.randn(320, 320, 3, 3, dtype=torch.bfloat16, device=device) / 54
        out, ref = torch.empty_like(x), F.conv2d(x, w, padding=1)
        ops.append(lambda: torch.mul(F.conv2d(x, w, padding=1), 1, out=out))
        checks.append((name, lambda: torch.equal(out, ref)))
    elif name == "sdpa":
        q, k, v = (torch.randn(1, 24, 1024, 128, dtype=torch.bfloat16, device=device) for _ in range(3))
        out, ref = torch.empty_like(q), F.scaled_dot_product_attention(q, k, v)
        ops.append(lambda: torch.mul(F.scaled_dot_product_attention(q, k, v), 1, out=out))
        checks.append((name, lambda: torch.equal(out, ref)))
    elif name in ("triton", "triton_pdl"):
        t = torch.zeros(1 << 16, dtype=torch.float32, device=device)
        extra = {"launch_pdl": True} if name == "triton_pdl" else {}
        def launch() -> None:
            launched(name, torch.cuda.current_stream())
            add_one[(t.numel() // 1024,)](t, t.numel(), BLOCK=1024, **extra)

        ops.append(launch)
        checks.append((name, lambda: bool((t == STEPS + 1).all())))
        SEEN[name] = t
    elif name == "parallel":
        side = torch.cuda.Stream(device=device)
        p = torch.zeros(64, dtype=torch.float32, device=device)

        def fork() -> None:
            current = torch.cuda.current_stream()
            side.wait_stream(current)
            with torch.cuda.stream(side):
                launched(name, side)
                p.add_(1)
            current.wait_stream(side)

        ops.append(fork)
        checks.append((name, lambda: bool((p == STEPS + 1).all())))
        SEEN[name] = p
    elif name == "legacy":  # 28892's shape: a launch that names no stream goes to the legacy stream
        p = torch.zeros(64, dtype=torch.float32, device=device)
        legacy = torch.cuda.default_stream(device)

        def stray() -> None:
            with torch.cuda.stream(legacy):
                launched(name, legacy)
                p.add_(1)

        ops.append(stray)
        checks.append((name, lambda: bool((p == STEPS + 1).all())))
        SEEN[name] = p
    elif name == "reduce":
        big = torch.randn(1 << 22, device=device)
        norm = torch.empty((), device=device)
        ops.append(lambda: torch.linalg.vector_norm(big, out=norm))
        checks.append((name, lambda: torch.equal(norm, torch.linalg.vector_norm(big))))
    else:
        raise ValueError(f"unknown variant {name}; one of {', '.join(VARIANTS)}")


def gpus() -> str:
    try:
        found = subprocess.run(["nvidia-smi", "--query-gpu=name,driver_version", "--format=csv,noheader"],
                               capture_output=True, text=True)
        return found.stdout.strip() or found.stderr.strip()
    except OSError as error:
        return f"nvidia-smi: {error}"


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    assert ctx.device.type == "cuda"
    device = torch.device("cuda", torch.cuda.current_device())
    torch.manual_seed(payload.seed)
    ops: list[Op] = []
    checks: list[Check] = []
    SEEN.clear()
    LAUNCHES.clear()
    for name in MIX if payload.variant == "mix" else (payload.variant,):
        part(name, device, ops, checks)
    counter = torch.zeros(64, dtype=torch.float32, device=device)

    def step(index: int, counter: torch.Tensor, label: str | None = None) -> None:
        del index, label
        for op in ops:
            op()
        counter.add_(1)

    labels = LABELS if payload.variant == "labels" else None
    for index in range(STEPS + 1):
        step(index, counter, labels[index] if labels else None)
    torch.cuda.synchronize()
    results = {name: bool(check()) for name, check in checks}
    seen = {name: dict(zip(*(v.tolist() for v in SEEN[name].unique(return_counts=True)))) for name in SEEN if not results.get(name, True)}
    results["counter"] = bool(torch.all(counter == STEPS + 1).item())
    detail = {"variant": payload.variant, "gpu": gpus(), "checks": results, "want": STEPS + 1, "seen": seen,
              "launches": {name: sorted(map(list, v)) for name, v in LAUNCHES.items()}}
    if not all(results.values()):
        raise RuntimeError(f"conditional body {json.dumps(detail, sort_keys=True)}")
    return Report(True, json.dumps(detail, sort_keys=True))
