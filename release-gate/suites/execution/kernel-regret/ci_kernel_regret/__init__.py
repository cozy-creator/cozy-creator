"""Every prebuilt GEMM tile and every attention candidate of this card's family, timed at product
shapes against the rule's own pick: GEMM config -1; a dense cell's first kernel that opens; a
sparse site's walk at each level (its sparse cells at that level and above, the first that opens)."""

import json
import math
import statistics
from collections.abc import Callable
from functools import partial

import msgspec
import torch
from cozy_runtime.author import App, AttentionLayout, Context, attention_scope
from cozy_runtime.internal import (
    arch,
    attention,
    attention_sol,
    kernel_cache,
    kernel_ready,
    prebuilt,
)
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
#: (tokens, heads, protected prefix) of a sparse step's Sol call: H3 Turbo 15 s on one GPU. Random
#: Q/K route about 16% of key blocks against H3's measured 22-23%, so times run low; the order holds.
SPARSE = {"h3": (109115, 56, 1259)}


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


def _sol_built(q: torch.Tensor) -> None:
    """Upstream Sol's CuTe object and preprocess kernels for this length, waited for: until they
    land, a sparse step serves its dense reference (`attention_sol._ready`, `_preprocessed`)."""
    card, store, index = attention_sol._capability(q.device), kernel_cache.configured(), q.device.index or 0
    if card is None or store is None:
        return
    tokens, heads, found = q.shape[1], q.shape[2], attention_sol._job_for(q.shape[1], card)
    kernel_ready.wait(f"{attention_sol.KERNEL} {found.key.digest[:16]}",
                      partial(attention_sol._open, store, found, card))  # fmt: skip
    kernel_ready.wait(f"{attention_sol.KERNEL} preprocess {tokens}x{heads} cuda:{index}",
                      partial(attention_sol._warm, index, tokens, heads))  # fmt: skip


def _sparse(row: arch.Row, ready: dict, shape: tuple[int, ...]) -> dict:
    """Each sparse candidate as a sparse step calls it (inside its model's layout and a Sol site),
    and per level the rule's pick (the walk's first that serves) against the walk's best."""
    tokens, heads, prefix = shape
    q, k, v = (torch.randn(1, tokens, heads, 128, device="cuda", dtype=torch.bfloat16) for _ in range(3))
    layout = AttentionLayout(live_tokens=tokens, protected_prefix=prefix, step=1, dense_until_step=1)
    calls, not_sparse = {}, {}
    site = attention_sol._SITE.set(attention_sol.Site("kernel-regret", "h3.attn"))
    try:
        with attention_scope(layout):
            for name, r in ready.items():
                if r.candidate.refusal(q, k, {}):
                    continue
                if r.candidate.import_name == "sol_attn":
                    _sol_built(q)
                call = partial(lambda i, m: dispatch_attention_fn(q, k, v, backend=m), m=r.member)
                with attention_sol.observing() as counts:
                    call(0)
                if counts["sparse"] != 1:  # it served a dense reference: not a sparse time
                    not_sparse[name] = dict(counts)
                    continue
                calls[name] = call
            ms = _bench(calls) if calls else {}
    finally:
        attention_sol._SITE.reset(site)
    family, levels = attention.SPARSE.get(row.family, {}), {}
    for at in attention.LEVELS:
        walk = [n for lv in attention.LEVELS[attention.LEVELS.index(at) :] for n in family.get(lv, ())]
        timed = [n for n in walk if n in ms]
        if at in family and timed:
            best = min(timed, key=ms.__getitem__)
            levels[at] = {"rule": timed[0], "best": best, "regret": round(ms[timed[0]] / ms[best] - 1, 4)}
    return {"ms": ms, "levels": levels, "not_sparse": not_sparse}


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
    row = arch.row(device.sm)
    names = list(dict.fromkeys(n for cell in attention.SPARSE.get(row.family, {}).values() for n in cell))
    ready, refused = {}, {}
    for name in names:
        try:
            ready[name] = attention.pinned(name, device)[0]
        except Exception as exc:
            refused[name] = f"{type(exc).__name__}: {exc}"[:300]
    if "sol-attn" in ready:  # its dense rows: what its walk binds at the family's 8-bit level
        walk = attention.walk(row, 8 if 8 in attention.DENSE.get(row.family, {}) else 16, layout=True)
        attention._bind_sol_dense(attention._sol_dense(walk[walk.index("sol-attn") + 1 :], device, {}, {}))
    report["sparse"] = []
    for label, shape in SPARSE.items():
        ctx.raise_if_cancelled()
        report["sparse"].append({"refused": refused, **_measured(label, partial(_sparse, row, ready, shape))})
    rows = [*(report["gemm"] if ops is not None else []), *report["attention"]]
    report["over_3pct"] = [
        f"{r.get('format', r.get('level'))} {r['shape']}: {r['regret']:.1%}, best {r['best']}"
        for r in rows
        if r.get("regret", 0) > 0.03
    ] + [
        f"sparse {at} {r['shape']}: {v['regret']:.1%}, rule {v['rule']}, best {v['best']}"
        for r in report["sparse"]
        for at, v in r.get("levels", {}).items()
        if v["regret"] > 0.03
    ]
    print("KERNEL-REGRET " + json.dumps(report, sort_keys=True), flush=True)
    return Report(True, json.dumps(report, sort_keys=True))
