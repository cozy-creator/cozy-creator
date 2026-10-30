# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.35", "diffusers>=0.40.0", "msgspec>=0.19", "torch==2.14.0"]
# ///
"""(torch is the pods' image's own, torch2.14.0-cu130: an environment that resolves another downloads it first.)
Every third-party kernel this machine's Runtime serves, through the Runtime's own paths, on inputs drawn from a fixed
CPU seed: the sha256 of each output's bytes, twice (a kernel that does not repeat its own bytes is `nondeterministic`
and is compared by its error instead), and its relative L2 against a float32 reference. Two Runtimes' reports, the
same card, compare op by op (`kernel-hash.sh`): equal digests are equal bytes.
Ops: each dense attention kernel of `attention.KERNELS` this card's row runs (Comfy Kitchen INT8, SageAttention3 FP4,
FA3, FA4 BF16/FP8, FlashInfer FP8), Kitchen's Sol and upstream Sol (CuTe DSL), Kitchen's int8 GEMM, the wheel's int8 and
fp8 GEMMs (CUTLASS) and, on FP4 rows, Kitchen's NVFP4 GEMM."""
import hashlib
import importlib.metadata as md
import json
import sys
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


#: The inputs' CPU seed: a constant, never a request field (a request's seed may be drawn per run).
SEED = 2026103003


class Request(msgspec.Struct, frozen=True):
    #: Comma-separated kernels or ops (`attn:sageattention3-fp4`, or a prefix ending in `:`, `gemm:`): run only these, or
    #: every op but these. A device fault ends the executor, so an op that may fault runs in its own request.
    only: str = ""
    skip: str = ""
    #: `sage3`: instead of the ops, SageAttention3's own steps one at a time (group mean, the three FP4 quantizers,
    #: the attention at each case its row's self-check runs), each named on stderr and synchronized, so a device
    #: fault names its step.
    parts: str = ""


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


def sage3_parts(dev: torch.device, index: int, device) -> dict[str, object]:
    """SageAttention3 (the wheel's) step by step, outside its row's self-check."""
    sage3 = import_module("cozy_runtime.internal.sage3")
    done: dict[str, object] = {}

    def step(name: str, fn) -> object:
        print(f"[kernel_hash] sage3 {name}", file=sys.stderr, flush=True)
        try:
            got = fn()
            torch.cuda.synchronize(dev)
            done[name] = "ok"
            return got
        except Exception as exc:
            done[name] = f"{type(exc).__name__}: {exc}"[:200]
            return None

    launch = step("group-mean launcher", lambda: triton_kernels.ready(sage3.KERNELS, True, device=f"cuda:{index}"))
    if launch is None:
        return done
    sage3.serve(index, launch)
    x = cpu(SEED, 1, 24, 512, 128).to(dev)
    step("group mean 1x24x512x128 bf16", lambda: sage3.triton_group_mean(x))
    step("group mean 1x16x512x64 bf16", lambda: sage3.triton_group_mean(cpu(SEED, 1, 16, 512, 64).to(dev)))
    step("quant", lambda: sage3.scale_and_quant_fp4(x))
    step("quant permute", lambda: sage3.scale_and_quant_fp4_permute(x))
    step("quant transpose", lambda: sage3.scale_and_quant_fp4_transpose(x))
    # (head dim, dtype, batch, query, keys, per-block mean): equal 128-multiples first, then the self-check's cases.
    for d, dtype, b, lq, lk, block in ((128, torch.bfloat16, 1, 512, 512, False), (128, torch.bfloat16, 1, 512, 512, True),
                                       (128, torch.bfloat16, 1, 257, 257, True), (128, torch.bfloat16, 2, 257, 257, True),
                                       (128, torch.bfloat16, 2, 300, 257, True), (128, torch.float16, 2, 300, 257, True),
                                       (64, torch.bfloat16, 2, 300, 257, True), (64, torch.float16, 2, 300, 257, True)):
        q = cpu(SEED + 1, b, 24, lq, d, dtype=dtype).to(dev)
        k, v = (cpu(SEED + 2 + i, b, 24, lk, d, dtype=dtype).to(dev) for i in range(2))
        step(f"attention d{d} {str(dtype)[6:]} b{b} q{lq} k{lk} {'block' if block else 'global'} mean",
             lambda: sage3.sageattn3_blackwell(q, k.clone(), v, per_block_mean=block))
    return done


def group_mean_parts(dev: torch.device, index: int) -> dict[str, object]:
    """Sage3's group mean at D=64 and D=128: Triton's own JIT launch (upstream's), then the Runtime set's launcher on
    the same inputs, their outputs compared; the launcher's D=64 entry last, its compile metadata said first."""
    sage3 = import_module("cozy_runtime.internal.sage3")
    done: dict[str, object] = {}

    def step(name: str, fn) -> object:
        print(f"[kernel_hash] group-mean {name}", file=sys.stderr, flush=True)
        try:
            got = fn()
            torch.cuda.synchronize(dev)
            done[name] = "ok"
            return got
        except Exception as exc:
            done[name] = f"{type(exc).__name__}: {exc}"[:200]
            return None

    def jit(q: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
        b, h, length, d = q.shape
        q_out, qm = torch.empty_like(q), torch.empty(b, h, length // 128, d, device=q.device, dtype=q.dtype)
        sage3.group_mean_kernel[(b, h, length // 128)](q, q_out, qm, b, h, length, d, *q.stride(), *qm.stride(), 128, 1)
        return q_out, qm

    inputs = {d: cpu(SEED, 1, 16, 512, d).to(dev) for d in (128, 64)}
    want = {d: step(f"jit d{d}", lambda d=d: jit(inputs[d])) for d in (128, 64)}
    launch = step("launcher", lambda: triton_kernels.ready(sage3.KERNELS, True, device=f"cuda:{index}"))
    if launch is None:
        return done
    sage3.serve(index, launch)
    for key, kernel in launch.compiled.items():
        meta = kernel.metadata
        line = f"{key}: shared {meta.shared}, warps {meta.num_warps}, scratch {getattr(meta, 'global_scratch_size', None)}"
        print(f"[kernel_hash] group-mean {line}", file=sys.stderr, flush=True)
        done[key] = line
    got = step("launcher d128", lambda: sage3.triton_group_mean(inputs[128]))
    if got is not None and want[128] is not None:
        done["d128 launcher == jit"] = bool(torch.equal(got[0], want[128][0]) and torch.equal(got[1], want[128][1]))
    got = step("launcher d64", lambda: sage3.triton_group_mean(inputs[64]))
    if got is not None and want[64] is not None:
        done["d64 launcher == jit"] = bool(torch.equal(got[0], want[64][0]) and torch.equal(got[1], want[64][1]))
    return done


@app.entrypoint()
def main(payload: Request) -> Result:
    index = torch.cuda.current_device()
    dev = torch.device("cuda", index)
    device = measure_device(torch, index)
    row = attention.device_row(device)
    ops: dict[str, dict[str, object]] = {}
    out: dict[str, object] = {"gpu": device.name, "sm": device.sm, "family": row.family,
                              "cozy_runtime": md.version("cozy-runtime"), "torch": torch.__version__, "ops": ops}

    only = tuple(x for x in payload.only.split(",") if x)
    skip = tuple(x for x in payload.skip.split(",") if x)

    def hits(name: str, items: tuple[str, ...]) -> bool:
        # An item names an op or a kernel whole (`attn:sageattention3-fp4` is not its `-fixed-means`); `x:` is a prefix.
        return any(name == i or name.startswith(i if i.endswith(":") else i + ":") for i in items)

    def wanted(name: str) -> bool:
        return (not only or hits(name, only)) and not hits(name, skip)

    def run(name: str, fn, ref: torch.Tensor | None = None) -> None:
        if not wanted(name):
            return
        # Each op is named on stderr before it launches: a device fault ends the executor, whose stderr the run keeps.
        print(f"[kernel_hash] {name}", file=sys.stderr, flush=True)
        try:
            first = fn()
            second = fn()
            entry: dict[str, object] = {"sha": digest(first), "repeats": digest(first) == digest(second)}
            if ref is not None:
                entry["rel_l2"] = rel(first, ref)
            ops[name] = entry
        except Exception as exc:
            ops[name] = {"error": f"{type(exc).__name__}: {exc}"[:240]}

    if payload.parts == "sage3":
        return Result(json.dumps({**out, "sage3": sage3_parts(dev, index, device)}, separators=(",", ":")))
    if payload.parts == "group-mean":
        return Result(json.dumps({**out, "group_mean": group_mean_parts(dev, index)}, separators=(",", ":")))
    started = time.perf_counter()
    # The inputs both arms must share: a differing digest here voids the comparison.
    out["inputs"] = {"q": digest(cpu(SEED, 1, 4608, 24, 128)), "x": digest(cpu(SEED + 20, 300, 4096)),
                     "w": digest(torch.randint(-127, 128, (4096, 4096), generator=torch.Generator().manual_seed(SEED + 21), dtype=torch.int8))}
    for name in DENSE:
        candidate = attention.BY_NAME[name]
        off = attention._off_row(candidate, row)
        if off or not wanted(f"attn:{name}"):
            continue
        print(f"[kernel_hash] open {name}", file=sys.stderr, flush=True)
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
            q, k, v = (cpu(SEED + i, b, s, h, d, dtype=dtype).to(dev) for i in range(3))
            run(f"attn:{name}:{b}x{s}x{h}x{d}:{str(dtype)[6:]}", lambda: forward(q, k, v), reference(q, k, v))
    # Sol: Kitchen's (INT8 rows) and upstream's (CuTe DSL), as the Runtime calls them.
    q, k, v = (cpu(SEED + 10 + i, 1, 16640, 24, 128).to(dev) for i in range(3))
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
        x = cpu(SEED + 20, m, kk).to(dev)
        data = torch.randint(-127, 128, (n, kk), generator=torch.Generator().manual_seed(SEED + 21), dtype=torch.int8).to(dev)
        scale = (cpu(SEED + 22, n, dtype=torch.float32).abs() + 0.5).mul(1e-3).to(dev)
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
            w8, s8 = prebuilt._fp8_law(torch, cpu(SEED + 23, n, kk, dtype=torch.float32, scale=0.02).to(dev))
            run(f"gemm:cozy-fp8:{m}x{n}x{kk}", lambda: lib.fp8_linear(x, w8, s8.view(-1), None, torch.bfloat16),
                x.float() @ (w8.float() * s8.view(-1)[:, None]).t() if m <= 300 else None)
    # Kitchen's NVFP4 GEMM, where the card's row has FP4 tensor cores.
    if row.fp4:
        n, kk = 4096, 4096
        g = torch.Generator().manual_seed(SEED + 30)
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
            x = cpu(SEED + 31, 1, 256, kk).to(dev)
            run("gemm:kitchen-nvfp4:256x4096x4096",
                lambda: nvfp4_linear.matmul(torch, x, weight, blocked, lambda: bscale, scale_2, torch.bfloat16))
    out["seconds"] = round(time.perf_counter() - started, 1)
    return Result(json.dumps(out, separators=(",", ":")))
