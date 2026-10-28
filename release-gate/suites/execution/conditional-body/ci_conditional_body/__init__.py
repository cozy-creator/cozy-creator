"""One node class inside the native conditional body `denoise_loop` records (IF -> WHILE -> body):
the first step runs eagerly, the rest natively, and the values are checked. One variant per run, so a
native fault ends only its executor and names its variant."""

import json
import subprocess

import msgspec
import torch
import torch.nn.functional as F
import triton
import triton.language as tl
from cozy_runtime.author import App, Context, denoise_loop
from cozy_runtime.internal import loop
from cuda.bindings import driver

app = App()
STEPS = 3  # native passes; the eager first step makes STEPS + 1
MIX = ("memset", "memcpy", "cublas", "cudnn", "sdpa", "triton")
VARIANTS = ("scalar", "memset", "memset32", "memcpy", "reduce", "parallel", "triton", "triton_pdl",
            "cublas", "cudnn", "sdpa", "mix")


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


def part(name, device, ops, checks):  # noqa: C901 - one table of small cases
    if name == "scalar":
        return
    if name in ("memset", "memset32"):
        scratch = torch.full((768,), 0xAB, dtype=torch.uint8, device=device)

        def memset(wide=name == "memset32"):
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
        cases = []
        for m, k, n in ((1, 3072, 18432), (8, 3072, 3072), (1024, 3072, 3072)):
            h = torch.randn(m, k, dtype=torch.bfloat16, device=device)
            w = torch.randn(k, n, dtype=torch.bfloat16, device=device) / k ** 0.5
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
        ops.append(lambda: add_one[(t.numel() // 1024,)](t, t.numel(), BLOCK=1024, **extra))
        checks.append((name, lambda: bool((t == STEPS + 1).all())))
    elif name == "parallel":
        side = torch.cuda.Stream(device=device)
        p = torch.zeros(64, dtype=torch.float32, device=device)

        def fork():
            current = torch.cuda.current_stream()
            side.wait_stream(current)
            with torch.cuda.stream(side):
                p.add_(1)
            current.wait_stream(side)

        ops.append(fork)
        checks.append((name, lambda: bool((p == STEPS + 1).all())))
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
    device = ctx.device
    torch.manual_seed(payload.seed)
    ops, checks = [], []
    for name in MIX if payload.variant == "mix" else (payload.variant,):
        part(name, device, ops, checks)
    counter = torch.zeros(64, dtype=torch.float32, device=device)

    def step(index, counter):
        del index
        for op in ops:
            op()
        counter.add_(1)

    denoise_loop(step, range(STEPS + 1), state=(counter,))
    torch.cuda.synchronize()
    results = {name: bool(check()) for name, check in checks}
    results["counter"] = bool(torch.all(counter == STEPS + 1).item())
    row = (loop.ROWS.get(ctx.request_id) or [{}])[-1]
    native = row.get("mode") == "resident" and row.get("native_steps") == STEPS
    detail = {"variant": payload.variant, "gpu": gpus(), "checks": results,
              "loop": {k: row.get(k) for k in ("mode", "native_steps", "refusals")}}
    if not native or not all(results.values()):
        raise RuntimeError(f"conditional body {json.dumps(detail, sort_keys=True)}")
    return Report(True, json.dumps(detail, sort_keys=True))
