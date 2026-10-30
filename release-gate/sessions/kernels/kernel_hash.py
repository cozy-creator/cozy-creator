# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.35", "diffusers>=0.40.0", "msgspec>=0.19", "torch>=2.14"]
# ///
"""Every third-party kernel this machine's Runtime serves, through the Runtime's own paths, on inputs drawn from a fixed
CPU seed: the sha256 of each output's bytes, twice (a kernel that does not repeat its own bytes is `nondeterministic`
and is compared by its error instead), and its relative L2 against a float32 reference. Two Runtimes' reports, the
same card, compare op by op (`kernel-hash.sh`): equal digests are equal bytes.
Ops: each dense attention kernel of `attention.KERNELS` this card's row runs (Comfy Kitchen INT8, SageAttention3 FP4,
FA3, FA4 BF16/FP8, FlashInfer FP8), Kitchen's Sol and upstream Sol (CuTe DSL), Kitchen's int8 GEMM, the wheel's int8 and
fp8 GEMMs (CUTLASS) and, on FP4 rows, Kitchen's NVFP4 GEMM."""
import hashlib
import importlib.metadata as md
import json
import time
from importlib import import_module

import msgspec
import torch

from cozy_runtime.author import App
from cozy_runtime.internal import arch, attention, int8_linear, kernel_ready, nvfp4_linear, prebuilt, triton_kernels  # noqa: F401  (binds kernel_ready.TRITON)
from cozy_runtime.internal.encoding.formats import _blocked_shape, to_blocked
from cozy_runtime.internal.encoding.selection import measure_device

app = App()
#: The dense kernels whose sources or builds the pins carry; SDPA, cuDNN and SageAttention 2 (unchanged pins) are not.
DENSE = ("kitchen-int8", "sageattention3-fp4", "sageattention3-fp4-global-lowmem", "sageattention3-fp4-fixed-means",
         "flash-attn3", "flash-attn4", "flash-attn4-fp8", "flashinfer-bf16-fp8")
#: (batch, tokens, heads, head_dim, dtype)
SHAPES = ((1, 4608, 24, 128, torch.bfloat16), (1, 1037, 16, 64, torch.bfloat16), (2, 2048, 24, 128, torch.float16))


class Request(msgspec.Struct, frozen=True):
    seed: int = 2026103003


class Result(msgspec.Struct, frozen=True):
    value: str


def digest(t: torch.Tensor) -> str:
    return hashlib.sha256(t.detach().contiguous().view(torch.uint8).cpu().numpy().tobytes()).hexdigest()[:24]


def rel(a: torch.Tensor, b: torch.Tensor) -> float:
    a, b = a.double().flatten(), b.double().flatten()
    return round(float((a - b).norm() / b.norm().clamp_min(1e-30)), 6)


def cpu(seed: int, *shape: int, dtype=torch.bfloat16, scale: float = 1.0) -> torch.Tensor:
    g = torch.Generator().manual_seed(seed)
    return (torch.randn(*shape, generator=g) * scale).to(dtype)


def reference(q: torch.Tensor, k: torch.Tensor, v: torch.Tensor) -> torch.Tensor:
    f = lambda t: t.float().transpose(1, 2)  # noqa: E731
    return torch.nn.functional.scaled_dot_product_attention(f(q), f(k), f(v)).transpose(1, 2)


@app.entrypoint()
def main(payload: Request) -> Result:
    index = torch.cuda.current_device()
    dev = torch.device("cuda", index)
    device = measure_device(torch, index)
    row = attention.device_row(device)
    ops: dict[str, dict[str, object]] = {}
    out: dict[str, object] = {"gpu": device.name, "sm": device.sm, "family": row.family,
                              "cozy_runtime": md.version("cozy-runtime"), "torch": torch.__version__, "ops": ops}

    def run(name: str, fn, ref: torch.Tensor | None = None) -> None:
        try:
            first = fn()
            second = fn()
            entry: dict[str, object] = {"sha": digest(first), "repeats": digest(first) == digest(second)}
            if ref is not None:
                entry["rel_l2"] = rel(first, ref)
            ops[name] = entry
        except Exception as exc:
            ops[name] = {"error": f"{type(exc).__name__}: {exc}"[:240]}

    started = time.perf_counter()
    for name in DENSE:
        candidate = attention.BY_NAME[name]
        off = attention._off_row(candidate, row)
        if off:
            continue
        try:
            ready = attention.pinned(name, device)[0]
            forward = attention._local(ready.candidate).forward
        except Exception as exc:
            ops[f"attn:{name}"] = {"error": f"{type(exc).__name__}: {exc}"[:240]}
            continue
        # Datacenter rows compile each CuTe shape per call site in-process: one shape keeps both arms in their guard.
        for b, s, h, d, dtype in SHAPES if row.family == arch.MMA_SYNC else SHAPES[:1]:
            if candidate.refusal(torch.empty(b, s, h, d, dtype=dtype, device="meta"),
                                 torch.empty(b, s, h, d, dtype=dtype, device="meta"), {}):
                continue
            q, k, v = (cpu(payload.seed + i, b, s, h, d, dtype=dtype).to(dev) for i in range(3))
            run(f"attn:{name}:{b}x{s}x{h}x{d}:{str(dtype)[6:]}", lambda: forward(q, k, v), reference(q, k, v))
    # Sol: Kitchen's (INT8 rows) and upstream's (CuTe DSL), as the Runtime calls them.
    q, k, v = (cpu(payload.seed + 10 + i, 1, 16640, 24, 128).to(dev) for i in range(3))
    try:
        int8_linear.kitchen_module(torch, index, True)
        kitchen = import_module("comfy_kitchen")
        run("sol:kitchen:16640", lambda: kitchen.sol_attn(q, k, v, tau=1.0, scale=None, sink_blocks=[0, 4],
                                                           sink_q=[0, 0], token_aug=0), reference(q, k, v))
    except Exception as exc:
        ops["sol:kitchen:16640"] = {"error": f"{type(exc).__name__}: {exc}"[:240]}
    try:
        attention._compiled(attention.BY_NAME["sol-attn"], device, True)
        sol = import_module("sol_attn")
        run("sol:upstream:16640", lambda: sol.sol_attn(q, k, v, tau=1.0, sink_tokens=256, sink_start=0), reference(q, k, v))
    except Exception as exc:
        ops["sol:upstream:16640"] = {"error": f"{type(exc).__name__}: {exc}"[:240]}
    # GEMMs: Kitchen's int8 (the leaves' fallback), the wheel's int8 and fp8 (CUTLASS).
    lib, why = prebuilt.library(torch, index, device.sm)
    for m, n, kk in ((3, 3072, 3072), (300, 4096, 4096), (16640, 3072, 3072), (16640, 16384, 3072), (16640, 3072, 8192)):
        x = cpu(payload.seed + 20, m, kk).to(dev)
        data = torch.randint(-127, 128, (n, kk), generator=torch.Generator().manual_seed(payload.seed + 21), dtype=torch.int8).to(dev)
        scale = (cpu(payload.seed + 22, n, dtype=torch.float32).abs() + 0.5).mul(1e-3).to(dev)
        try:
            module = int8_linear.kitchen_module(torch, index, True)[0]
            run(f"gemm:kitchen-int8:{m}x{n}x{kk}", lambda: module.int8_linear(x, data, scale, None, torch.bfloat16,
                                                                              convrot=True, convrot_groupsize=256))
        except Exception as exc:
            ops[f"gemm:kitchen-int8:{m}x{n}x{kk}"] = {"error": f"{type(exc).__name__}: {exc}"[:240]}
        if lib is None:
            ops["gemm:cozy"] = {"error": why[:240]}
            continue
        run(f"gemm:cozy-int8:{m}x{n}x{kk}", lambda: lib.int8_linear(x, data, scale, None, torch.bfloat16))
        if row.fp8:
            w8, s8 = prebuilt._fp8_law(torch, cpu(payload.seed + 23, n, kk, dtype=torch.float32, scale=0.02).to(dev))
            run(f"gemm:cozy-fp8:{m}x{n}x{kk}", lambda: lib.fp8_linear(x, w8, s8.view(-1), None, torch.bfloat16),
                x.float() @ (w8.float() * s8.view(-1)[:, None]).t() if m <= 300 else None)
    # Kitchen's NVFP4 GEMM, where the card's row has FP4 tensor cores.
    if row.fp4:
        n, kk = 4096, 4096
        g = torch.Generator().manual_seed(payload.seed + 30)
        weight = torch.randint(0, 256, (n, kk // 2), generator=g, dtype=torch.uint8).to(dev)
        bscale = (torch.rand(n, kk // 16, generator=g) * 2 + 0.05).to(dev).to(torch.float8_e4m3fn)
        scale_2 = torch.tensor([0.02], device=dev)
        blocked = to_blocked(torch, bscale).view(*_blocked_shape(n, kk // 16))
        name, waited = f"{nvfp4_linear.KITCHEN} cuda:{index}", time.perf_counter()
        while nvfp4_linear.kitchen(torch, index, False) is None and not kernel_ready.failed(name) and time.perf_counter() - waited < 300:
            kernel_ready.boundary()
            time.sleep(1)
        if nvfp4_linear.kitchen(torch, index, False) is None:
            ops["gemm:kitchen-nvfp4"] = {"error": f"does not serve: {kernel_ready.failed(name) or 'timed out'}"[:240]}
        else:
            x = cpu(payload.seed + 31, 1, 256, kk).to(dev)
            run("gemm:kitchen-nvfp4:256x4096x4096",
                lambda: nvfp4_linear.matmul(torch, x, weight, blocked, lambda: bscale, scale_2, torch.bfloat16))
    out["seconds"] = round(time.perf_counter() - started, 1)
    return Result(json.dumps(out, separators=(",", ":")))
