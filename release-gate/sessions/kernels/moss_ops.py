# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.35", "msgspec>=0.19", "torch>=2.14"]
# ///
"""MOSS-Image's DiT block, op by op, on this machine's GPU: the Runtime's ops as moss-image runs them against the
Comfy Kitchen ops ComfyUI's MOSS-Image runs, at the 4 MP shapes (16,384 image tokens and a prompt's, width 3072,
24 heads of 128, FFN 8192 gated). Random int8.convrot weights; no model. Per op: median ms over `iters` CUDA-event
timed calls after warm-up, and the relative L2 between the two engines' outputs (each GEMM also against an f64 law
on a row sample). Each GEMM shape also runs every config of the Runtime's tile table. A block's sum, times the 83
block calls of a 50-step CFG step (42 blocks, the negative skips one), is each engine's projected DiT time a step."""
import importlib.metadata as md
import json
import statistics
from importlib import import_module

import msgspec
import torch

from cozy_runtime.author import App
from cozy_runtime.author.kernels import rms_rope_split_half
from cozy_runtime.internal import int8_linear, prebuilt, triton_kernels  # noqa: F401  (binds kernel_ready.TRITON)
from cozy_runtime.internal.attention_upstream import kitchen_int8
from cozy_runtime.internal.encoding.formats import hadamard256

app = App()
DIM, FFN, HEADS, HEAD_DIM, BLOCK_CALLS = 3072, 8192, 24, 128, 83


class Request(msgspec.Struct, frozen=True):
    tokens: int = 16384 + 256
    iters: int = 20
    seed: int = 2026103002


class Result(msgspec.Struct, frozen=True):
    value: str


def timed(fn, iters: int) -> float:
    for _ in range(3):
        fn()
    times = []
    for _ in range(iters):
        start, end = torch.cuda.Event(enable_timing=True), torch.cuda.Event(enable_timing=True)
        start.record()
        fn()
        end.record()
        end.synchronize()
        times.append(start.elapsed_time(end))
    return round(statistics.median(times), 4)


def rel(a: torch.Tensor, b: torch.Tensor) -> float:
    a, b = a.double().flatten(), b.double().flatten()
    return float((a - b).norm() / b.norm().clamp_min(1e-30))


def law(x: torch.Tensor, data: torch.Tensor, scale: torch.Tensor, rows: int = 256) -> torch.Tensor:
    """The unquantized product of `rows` sampled rows: rotated x times the stored rotated weight, in f64."""
    pick = torch.linspace(0, x.shape[0] - 1, rows, device=x.device).long()
    return (hadamard256(torch, x[pick].double()) / 16) @ (data.double() * scale.double()[:, None]).t(), pick


@app.entrypoint()
def main(payload: Request) -> Result:
    torch.manual_seed(payload.seed)
    dev, index = torch.device("cuda"), torch.cuda.current_device()
    sm = int(int8_linear.capability(torch, index))
    props = torch.cuda.get_device_properties(index)
    out: dict[str, object] = {
        "gpu": props.name, "sm": sm, "sms": props.multi_processor_count, "l2_mib": props.L2_cache_size >> 20,
        "cozy_runtime": md.version("cozy-runtime"), "torch": torch.__version__, "tokens": payload.tokens,
    }
    ops, detail = prebuilt.library(torch, index, sm)
    if ops is None:
        raise RuntimeError(f"the Runtime's int8 op does not serve sm{sm}: {detail}")
    found = int8_linear.kitchen_module(torch, index, True)
    if found is None:
        raise RuntimeError("comfy-kitchen did not build here")
    ck = import_module("comfy_kitchen")  # importable once kitchen_module placed the Runtime's reviewed build, as attention_upstream imports it

    out["kitchen"] = getattr(ck, "__version__", "") or found[1][:16]
    m, it = payload.tokens, payload.iters
    bf = torch.bfloat16

    def weight(n: int, k: int) -> tuple[torch.Tensor, torch.Tensor]:
        return (torch.randint(-127, 128, (n, k), dtype=torch.int8, device=dev),
                torch.rand(n, device=dev).add_(0.5).mul_(2e-3))

    w = {"q": weight(DIM, DIM), "k": weight(DIM, DIM), "v": weight(DIM, DIM), "o": weight(DIM, DIM),
         "up_gate": weight(2 * FFN, DIM), "down": weight(DIM, FFN)}
    x = torch.randn(1, m, DIM, device=dev, dtype=bf)
    hidden = torch.randn(1, m, 2 * FFN, device=dev, dtype=bf) * 3
    norm_w = (1 + 0.1 * torch.randn(DIM, device=dev)).to(bf)
    scale_t, shift_t = (0.2 * torch.randn(1, DIM, device=dev)).to(bf), (0.2 * torch.randn(1, DIM, device=dev)).to(bf)
    folded = (norm_w.float() * (1 + scale_t.float()) - 1).to(bf)
    qn, kn = (1 + 0.1 * torch.randn(HEAD_DIM, device=dev)).to(bf), (1 + 0.1 * torch.randn(HEAD_DIM, device=dev)).to(bf)
    phase = torch.rand(m, HEAD_DIM // 2, device=dev) * 50
    rope = torch.stack((phase.cos(), -phase.sin(), phase.sin(), phase.cos()), -1).unflatten(-1, (2, 2))[None, :, None]
    q4 = torch.randn(1, m, HEADS, HEAD_DIM, device=dev, dtype=bf)
    k4, v4 = torch.randn_like(q4), torch.randn_like(q4)
    ops_ms: dict[str, dict[str, object]] = {}

    def row(name: str, cozy, kitchen, count: int = 1, check: bool = True) -> None:
        entry: dict[str, object] = {"count": count}
        for engine, fn in (("cozy", cozy), ("kitchen", kitchen)):
            try:
                entry[engine] = timed(fn, it)
            except Exception as exc:
                entry[engine] = f"{type(exc).__name__}: {exc}"[:300]
        if check:
            try:
                a, b = cozy(), kitchen()
                a, b = (a[0], b[0]) if isinstance(a, tuple) else (a, b)
                entry["rel_l2"] = round(rel(a, b), 6)
            except Exception as exc:
                entry["rel_l2"] = f"{type(exc).__name__}: {exc}"[:300]
        ops_ms[name] = entry

    # norm1 / norm2: ours quantizes the modulated rows once for q, k, v (and for ffn_up_gate); Kitchen writes bf16 rows.
    row("modulated_norm", lambda: ops.int8_quantize(x.view(m, DIM), folded, shift_t, 2, 1e-6, 0),
        lambda: ck.rms_adaln(x, folded, shift_t, 1e-6), count=2, check=False)
    rows_q, rows_s = ops.int8_quantize(x.view(m, DIM), folded, shift_t, 2, 1e-6, 0)
    y = ck.rms_adaln(x, folded, shift_t, 1e-6)
    # q, k, v: ours three GEMMs of the shared rows; Kitchen three int8_linear, each quantizing y.
    row("qkv_one", lambda: ops.int8_gemm(rows_q, rows_s, *w["q"], None, bf),
        lambda: ck.int8_linear(y, *w["q"], None, bf, convrot=True, convrot_groupsize=256), count=3)
    row("rms_rope", lambda: rms_rope_split_half(q4, k4, rope, qn, kn, 1e-6),
        lambda: ck.rms_rope_split_half(q4, k4, rope, qn, kn, 1e-6))
    t = lambda z: z.transpose(1, 2)  # noqa: E731  [B, S, H, D] -> [B, H, S, D], as both engines hand Kitchen

    def prequantized():
        return ck.int8_attention_from_prequantized(ck.prequantize_int8_attention(t(q4), t(k4), t(v4)))

    row("attention", lambda: kitchen_int8(q4, k4, v4), lambda: t(prequantized()))
    attn = kitchen_int8(q4, k4, v4).flatten(2)
    row("o", lambda: ops.int8_linear(attn, *w["o"], None, bf),
        lambda: ck.int8_linear(attn, *w["o"], None, bf, convrot=True, convrot_groupsize=256))
    row("up_gate", lambda: ops.int8_gemm(rows_q, rows_s, *w["up_gate"], None, bf),
        lambda: ck.int8_linear(y, *w["up_gate"], None, bf, convrot=True, convrot_groupsize=256))
    # ffn_down: ours SiTU folded into its own quantization, then the GEMM; Kitchen SiTU as int8_linear's input_act.
    row("situ_down", lambda: ops.int8_gemm(*ops.int8_quantize(hidden.view(m, 2 * FFN), None, None, 0, 0.0, 3), *w["down"], None, bf),
        lambda: ck.int8_linear(hidden, *w["down"], None, bf, convrot=True, convrot_groupsize=256, input_act="situ"))
    out["ops_ms"] = ops_ms
    # The split of ours: a quantize alone, each GEMM alone at its config (-1 is the shipped choice).
    split: dict[str, object] = {"quantize_3072": timed(lambda: ops.int8_quantize(attn.view(m, DIM)), it),
                                "situ_quantize": timed(lambda: ops.int8_quantize(hidden.view(m, 2 * FFN), None, None, 0, 0.0, 3), it)}
    situ_q, situ_s = ops.int8_quantize(hidden.view(m, 2 * FFN), None, None, 0, 0.0, 3)
    for name, (qa, sa) in (("qkv_one", (rows_q, rows_s)), ("o", (rows_q, rows_s)), ("up_gate", (rows_q, rows_s)),
                           ("down", (situ_q, situ_s))):
        configs: dict[str, object] = {}
        for config in range(-1, 7):
            try:
                configs[str(config)] = timed(lambda: ops.int8_gemm(qa, sa, *w[name if name != "qkv_one" else "q"], None, bf, config), it)
            except Exception as exc:
                configs[str(config)] = f"{type(exc).__name__}: {exc}"[:120]
        split[f"gemm_{name}"] = configs
    out["cozy_gemm_configs_ms"] = split
    # Each engine's GEMMs against the f64 law on sampled rows (a shared sanity bar for both).
    xs = attn.view(m, DIM)
    want, pick = law(xs, *w["o"])
    out["o_vs_law"] = {"cozy": round(rel(ops.int8_linear(xs, *w["o"], None, bf)[pick], want), 6),
                       "kitchen": round(rel(ck.int8_linear(xs, *w["o"], None, bf, convrot=True, convrot_groupsize=256)[pick], want), 6)}
    block = {e: round(sum(v[e] * v["count"] for v in ops_ms.values() if isinstance(v[e], float)), 3) for e in ("cozy", "kitchen")}
    out["block_ms"] = block
    out["step_s_projected"] = {e: round(v * BLOCK_CALLS / 1000, 3) for e, v in block.items()}
    out["peak_mib"] = torch.cuda.max_memory_allocated() >> 20
    return Result(json.dumps(out, separators=(",", ":")))
