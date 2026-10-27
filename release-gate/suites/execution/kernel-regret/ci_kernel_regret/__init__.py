"""Every prebuilt GEMM tile and every dense attention candidate of this card's family, timed at
product shapes against the rule's own pick (GEMM config -1; a cell's first kernel that opens)."""

import json
import math
import statistics
from collections.abc import Callable
from functools import partial

import msgspec
import torch
from cozy_runtime.author import App, Context
from cozy_runtime.internal import arch, attention, prebuilt
from cozy_runtime.internal.encoding.selection import measure_device
from diffusers.models.attention_dispatch import dispatch_attention_fn

app = App()
#: M, then (N, K) of qkv/out, FFN up and FFN down: SDXL 1024² (CFG batch 2) at 64² and 32²,
#: Qwen-Image 1024², H3 15 s on one GPU
GEMMS = {
    "sdxl64": (8192, (640, 640), (5120, 640), (640, 2560)),
    "sdxl32": (2048, (1280, 1280), (10240, 1280), (1280, 5120)),
    "qwen": (8192, (3072, 3072), (12288, 3072), (3072, 12288)),
    "h3": (111334, (7168, 7168), (28672, 7168), (7168, 28672)),
}
#: (batch, tokens, heads, head dim): the same models' attention; H3 as one rank of four
ATTENTION = {"sdxl64": (2, 4096, 10, 64), "sdxl32": (2, 1024, 20, 64),
             "qwen": (1, 4352, 24, 128), "h3": (1, 111334, 14, 128)}  # fmt: skip


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 202610270321


class Report(msgspec.Struct):
    passed: bool
    detail: str


def _bench(calls: dict[str, Callable[[int], object]], rounds: int = 7) -> dict[str, float]:
    """Median device ms per call, in rounds whose order rotates; each sample runs >= 2 ms."""
    reps, samples, names = {}, {n: [] for n in calls}, list(calls)
    for round_ in range(-1, rounds):  # round -1 warms each and sizes its sample
        for name in names[round_ % len(names) :] + names[: round_ % len(names)]:
            start, end = torch.cuda.Event(enable_timing=True), torch.cuda.Event(enable_timing=True)
            if round_ < 0:
                calls[name](0)
            start.record()
            for i in range(reps.get(name, 1)):
                calls[name](i)
            end.record()
            end.synchronize()
            took = start.elapsed_time(end) / reps.get(name, 1)
            if round_ < 0:
                reps[name] = min(256, max(1, math.ceil(2 / max(took, 1e-3))))
            else:
                samples[name].append(took)
    return {n: round(statistics.median(v), 4) for n, v in samples.items()}


def _bytes(*shape: int, fp8: bool) -> torch.Tensor:
    """Random operands; E4M3 bytes masked off NaN."""
    b = torch.randint(0, 256, shape, dtype=torch.uint8, device="cuda")
    return (b & 0x77).view(torch.float8_e4m3fn) if fp8 else b.view(torch.int8)


def _gemm(gemm: Callable[..., torch.Tensor], fp8: bool, m: int, n: int, k: int, l2: int) -> dict:
    """The rule against every tile the card's table holds at (M, N, K), and their bits."""
    q, xs, scale = _bytes(m, k, fp8=fp8), torch.rand(m, device="cuda"), torch.rand(n, device="cuda")
    # weight copies over twice the L2: no call reads its weights warm, as no served call does
    ws = [_bytes(n, k, fp8=fp8) for _ in range(-(-2 * l2 // (n * k)))]

    def run(config: int) -> Callable[[int], torch.Tensor]:
        return lambda i: gemm(q, xs, ws[i % len(ws)], scale, None, torch.bfloat16, config)

    rule, calls, same = run(-1)(0), {"rule": run(-1)}, {}
    for config in range(8):
        try:
            same[config] = torch.equal(run(config)(0), rule)
        except ValueError:  # not in this card's table, or not this shape
            continue
        calls[str(config)] = run(config)
    del rule
    ms = _bench(calls)
    best = min((c for c in ms if c != "rule"), key=ms.__getitem__)
    regret = round(ms["rule"] / ms[best] - 1, 4)
    return {"ms": ms, "best": best, "regret": regret, "same_bits": same}


def _attend(ready: dict, shape: tuple[int, ...]) -> dict:
    """The cell's kernels at one shape, in the cell's order: the first is the rule's pick."""
    q, k, v = (torch.randn(*shape, device="cuda", dtype=torch.bfloat16) for _ in range(3))
    calls = {name: (lambda i, m=r.member: dispatch_attention_fn(q, k, v, backend=m))
             for name, r in ready.items() if not r.candidate.refusal(q, k, {})}  # fmt: skip
    if not calls:
        return {"ms": {}}
    ms = _bench(calls)
    best = min(ms, key=ms.__getitem__)
    return {"ms": ms, "best": best, "regret": round(next(iter(ms.values())) / ms[best] - 1, 4)}


def _measured(what: str, measure: Callable[[], dict]) -> dict:
    try:
        return {"shape": what, **measure()}
    except torch.OutOfMemoryError:
        return {"shape": what, "skipped": "out of memory"}
    finally:
        torch.cuda.empty_cache()


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    torch.manual_seed(payload.seed)
    index = torch.cuda.current_device()
    props, device = torch.cuda.get_device_properties(index), measure_device(torch, index)
    ops, why = prebuilt.library(torch, index, device.sm)
    report: dict = {"card": device.name, "sm": device.sm, "driver": device.driver,
                    "sms": props.multi_processor_count, "l2": props.L2_cache_size, "gemm": why,
                    "attention": []}  # fmt: skip
    if ops is not None:
        report["gemm"] = []
        for f in (prebuilt.INT8, prebuilt.FP8) if ops.cutlass_fp8 else (prebuilt.INT8,):
            gemm = ops.fp8_gemm if f.fp8 else ops.int8_gemm
            for model, (m, *sites) in GEMMS.items():
                for site, (n, k) in zip(("qkv", "up", "down"), sites, strict=True):
                    ctx.raise_if_cancelled()
                    row = {"format": f.name, "shape": f"{model} {site}"}
                    if k % f.multiple:  # the op takes K in whole groups (int8: ConvRot's 256)
                        report["gemm"].append({**row, "skipped": f"K {k} % {f.multiple}"})
                        continue
                    measure = partial(_gemm, gemm, f.fp8, m, n, k, props.L2_cache_size)
                    report["gemm"].append({**row, **_measured(row["shape"], measure)})
    for level, names in attention.DENSE.get(arch.row(device.sm).family, {}).items():
        ready, refused = {}, {}
        for name in names:
            try:
                ready[name] = attention.pinned(name, device)[0]
            except Exception as exc:  # a kernel this card does not open is a finding
                refused[name] = f"{type(exc).__name__}: {exc}"[:300]
        for label, shape in ATTENTION.items():
            ctx.raise_if_cancelled()
            row = _measured(label, partial(_attend, ready, shape))
            report["attention"].append({"level": level, "refused": refused, **row})
    rows = [*(report["gemm"] if ops is not None else []), *report["attention"]]
    report["over_3pct"] = [
        f"{r.get('format', r.get('level'))} {r['shape']}: {r['regret']:.1%}, best {r['best']}"
        for r in rows
        if r.get("regret", 0) > 0.03
    ]
    print("KERNEL-REGRET " + json.dumps(report, sort_keys=True), flush=True)
    return Report(True, json.dumps(report, sort_keys=True))
