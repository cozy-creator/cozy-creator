"""One variant in its own process: PreparedRepeat composes a step holding that node class into its
native IF -> WHILE conditional graph, runs it once, and the values are checked. A native fault ends
only this process.
    child.py <variant> <seed>"""

import json
import resource
import sys

import torch
import torch.nn.functional as F
import triton
import triton.language as tl
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.internal import accel
from cuda.bindings import driver

PREFIX = "[conditional-body] "
STEPS = 3
MIX = ("memset", "memcpy", "cublas", "cudnn", "sdpa", "triton")


@triton.jit
def add_one(pointer, count, BLOCK: tl.constexpr):
    offsets = tl.program_id(0) * BLOCK + tl.arange(0, BLOCK)
    mask = offsets < count
    tl.store(pointer + offsets, tl.load(pointer + offsets, mask=mask) + 1.0, mask=mask)


def emit(**row):
    print(PREFIX + json.dumps(row), flush=True)


def part(name, device, ops, checks, resets):  # noqa: C901 - one table of small cases
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
        resets.append(lambda: scratch.fill_(0xAB))
        checks.append((name, lambda: bool((scratch == 0x5A).all())))
    elif name == "memcpy":
        a = torch.randn(8192, device=device)
        b = torch.zeros_like(a)
        ops.append(lambda: b.copy_(a))
        resets.append(b.zero_)
        checks.append((name, lambda: torch.equal(a, b)))
    elif name == "cublas":
        cases = []
        for m, k, n in ((1, 3072, 18432), (8, 3072, 3072), (1024, 3072, 3072)):
            h = torch.randn(m, k, dtype=torch.bfloat16, device=device)
            w = torch.randn(k, n, dtype=torch.bfloat16, device=device) / k ** 0.5
            cases.append((h, w, torch.empty(m, n, dtype=torch.bfloat16, device=device), torch.mm(h, w)))
        ops.append(lambda: [torch.mm(h, w, out=o) for h, w, o, _ in cases])
        resets.extend(o.zero_ for _, _, o, _ in cases)
        checks.append((name, lambda: all(torch.equal(o, r) for _, _, o, r in cases)))
    elif name == "cudnn":
        x = torch.randn(1, 320, 64, 64, dtype=torch.bfloat16, device=device)
        w = torch.randn(320, 320, 3, 3, dtype=torch.bfloat16, device=device) / 54
        out, ref = torch.empty_like(x), F.conv2d(x, w, padding=1)
        ops.append(lambda: torch.mul(F.conv2d(x, w, padding=1), 1, out=out))
        resets.append(out.zero_)
        checks.append((name, lambda: torch.equal(out, ref)))
    elif name == "sdpa":
        q, k, v = (torch.randn(1, 24, 1024, 128, dtype=torch.bfloat16, device=device) for _ in range(3))
        out, ref = torch.empty_like(q), F.scaled_dot_product_attention(q, k, v)
        ops.append(lambda: torch.mul(F.scaled_dot_product_attention(q, k, v), 1, out=out))
        resets.append(out.zero_)
        checks.append((name, lambda: torch.equal(out, ref)))
    elif name in ("triton", "triton_pdl"):
        t = torch.zeros(1 << 16, dtype=torch.float32, device=device)
        extra = {"launch_pdl": True} if name == "triton_pdl" else {}
        ops.append(lambda: add_one[(t.numel() // 1024,)](t, t.numel(), BLOCK=1024, **extra))
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
    elif name == "reduce":
        big = torch.randn(1 << 22, device=device)
        norm = torch.empty((), device=device)
        ops.append(lambda: torch.linalg.vector_norm(big, out=norm))
        resets.append(norm.zero_)
        checks.append((name, lambda: torch.equal(norm, torch.linalg.vector_norm(big))))
    else:
        raise ValueError(f"unknown variant {name}")


def main(variant, seed):
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))  # a crash leaves no multi-GB core on the pod
    accel.initialize(torch, "cuda")  # the executor's allocator settings and hook
    device = torch.device("cuda", torch.cuda.current_device())
    torch.manual_seed(seed)
    ops, checks, resets = [], [], []
    with torch.no_grad():
        for name in MIX if variant == "mix" else (variant,):
            part(name, device, ops, checks, resets)
        counter = torch.zeros(64, dtype=torch.float32, device=device)

        def step(index):
            del index
            for op in ops:
                op()
            counter.add_(1)

        emit(stage="built")
        plan = PreparedRepeat.capture(step, state=(counter,), steps=STEPS)
        emit(stage="constructed")
        try:
            for reset in resets:
                reset()
            torch.cuda.synchronize()
            (final,) = plan.submit().wait()
            torch.cuda.synchronize()
            results = {name: bool(check()) for name, check in checks}
            results["counter"] = bool(torch.all(final == STEPS).item())
        finally:
            plan.close()
    emit(stage="executed", checks=results, passed=all(results.values()))


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]))
