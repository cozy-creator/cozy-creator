"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up).
- `quality`: one lane's top-1 next tokens and perplexity on `TEXTS`, compared across calls.
- `batch_shapes`: batch-shapes raced on torch's default stream against the same cases settled (cozy-runtime#2309)."""

import json
import time
from collections.abc import Callable

import msgspec
import torch
from cozy_runtime import lm
from cozy_runtime.author import App, Context
from cozy_runtime.vlm import VisionLM
from torch.overrides import TorchFunctionMode

app = App()

#: Plain English and code, a few hundred tokens each: what a caption and an answer read like.
TEXTS = (
    "The lighthouse stood at the end of a long stone pier, its white tower streaked with rust where the iron rail met the "
    "wall. Every evening the keeper climbed the spiral stairs, counted the steps out of habit, and wiped the salt from the "
    "great lens before lighting the lamp. Ships passing in the dark never saw him, only the slow sweep of the beam across "
    "the water, but the fishermen in the harbour town said they could set their clocks by it.",
    "A photograph of a red-brick railway station at dusk: two platforms under a glass canopy, a green train waiting with "
    "its doors open, a few passengers with suitcases near a bench, and a clock above the entrance reading ten past six. "
    "The sky behind the canopy is orange fading to violet, and the platform lamps have just come on.",
    "def merge(a, b):\n    out = []\n    i = j = 0\n    while i < len(a) and j < len(b):\n        if a[i] <= b[j]:\n"
    "            out.append(a[i]); i += 1\n        else:\n            out.append(b[j]); j += 1\n    return out + a[i:] + b[j:]\n",
    "Quantization stores each weight in fewer bits: a block of thirty-two values shares one scale, and each value keeps an "
    "eight-bit integer. Decoding multiplies them back. The error is small compared with the spread of the weights, so the "
    "model's answers barely change, while the memory it reads for every token halves.",
)


#: ggml-decode's story, as Qwen's chat template writes a user turn with thinking off: the same tokens llama.cpp's side
#: is given (fixtures/ggml-decode/llama.sh).
PROMPT = (
    "<|im_start|>user\nWrite a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep "
    "writing until you are stopped.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
)


class Agreement(msgspec.Struct, frozen=True):
    detail: str


def _logprobs(model: VisionLM, ids: torch.Tensor) -> torch.Tensor:
    net = model.pipe.components["model"]
    with torch.inference_mode():
        out = net(input_ids=ids.to(net.device), use_cache=False)
    return torch.log_softmax(out.logits[0].float(), dim=-1).cpu()


@app.entrypoint(accelerator=True)
def logits(ctx: Context, reference: VisionLM, model: VisionLM) -> Agreement:
    """Each text's next-token distributions under both lanes: top-1 agreement and KL(reference || model)."""
    tokenizer = reference.processor.tokenizer
    rows = []
    for text in TEXTS:
        ids = torch.tensor([tokenizer(text)["input_ids"]])
        want = _logprobs(reference, ids)
        got = _logprobs(model, ids)
        kl = (want.exp() * (want - got)).sum(-1)
        rows.append({
            "tokens": int(ids.shape[1]),
            "top1": float((want.argmax(-1) == got.argmax(-1)).float().mean()),
            "kl_mean": float(kl.mean()),
            "kl_p99": float(kl.quantile(0.99)),
            "kl_max": float(kl.max()),
        })
    return Agreement(json.dumps(rows))


@app.entrypoint(accelerator=True)
def quality(ctx: Context, model: VisionLM) -> Agreement:
    """Each of `TEXTS` under this lane alone: its top-1 next token at every position and its perplexity of the text
    (exp of the mean negative log-likelihood of each next token). Two lanes' top-1 agreement is read from two such calls,
    so a lane and its bf16 reference never share a card."""
    tokenizer = model.processor.tokenizer
    rows = []
    for text in TEXTS:
        ids = torch.tensor([tokenizer(text)["input_ids"]])
        logp = _logprobs(model, ids)
        nll = -logp[:-1].gather(-1, ids[0, 1:, None]).squeeze(-1)
        rows.append({"tokens": int(ids.shape[1]), "top1": logp.argmax(-1).tolist(), "ppl": float(nll.mean().exp())})
    return Agreement(json.dumps(rows))


#: batch-shapes' cases (cozy-runtime#2291 `internal/bench/batch_shapes.py`): Qwen3.5-9B's MLP (site, out, in), the 9B's
#: q8_0 and Q4_K_M's q4_k, decode batches and prefill chunks each held against its group's largest.
SHAPE_SITES = (("up", 12288, 4096), ("down", 4096, 12288))
SHAPE_SCHEMES = ("ggml.q8_0", "ggml.q4_k")
SHAPE_GROUPS = (("decode", (1, 2, 4, 8, 13, 16)), ("prefill", (640, 1500, 4096)))


def _shapes(raced: bool) -> dict[str, dict[str, object]]:
    """batch-shapes' rows, keyed: the same rows through the leaf (`linear`) and as one launch (`matmul`) in each batch,
    against the group's largest. `raced`: as its first form ran (red in 44407876384, 44407881866), each call on torch's
    default stream and read at once; else as cozy-runtime `2bc92f3bd` settled it, each call on its own stream, synced and
    copied before a read, with the largest called twice (`repeat`: a call that differs from itself)."""
    from cozy_runtime.internal import ggml_linear

    deadline = time.monotonic() + 120
    while (ops := ggml_linear.native(torch, 0)) is None:
        if why := ggml_linear.why(0) or ggml_linear.carried(ggml_linear.capability(torch, 0)):
            raise RuntimeError(f"ggml kernels not served: {why}")
        if time.monotonic() > deadline:
            raise RuntimeError("the card's ggml kernels were not verified in 120 s")
        time.sleep(0.1)
    own = torch.cuda.Stream()

    def run(call: Callable[[torch.Tensor], torch.Tensor], r: torch.Tensor) -> torch.Tensor:
        if raced:
            return call(r)
        with torch.cuda.stream(own):
            out = call(r)
        own.synchronize()
        held = out.clone()
        torch.cuda.synchronize()
        return held

    def row(got: torch.Tensor, against: torch.Tensor) -> dict[str, object]:
        diff = float((got - against).abs().max())
        return {"rows_identical": int((got == against).all(dim=1).sum()), "max_abs_diff": diff if diff == diff else "nan"}

    made = torch.Generator(device="cpu").manual_seed(213)
    rows: dict[str, dict[str, object]] = {}
    with torch.cuda.stream(torch.cuda.default_stream()):
        for scheme in SHAPE_SCHEMES:
            for site, n, k in SHAPE_SITES:
                weight = torch.randn((n, k), generator=made).mul_(0.02)
                blocks = ggml_linear.encode(torch, scheme, weight).to("cuda")
                x = torch.randn((4096, k), generator=made).to("cuda", torch.bfloat16)
                paths = {
                    "leaf": lambda r: ggml_linear.linear(torch, r, blocks, scheme, n, None, torch.float32),
                    "launch": lambda r: ggml_linear.matmul(ops, torch, r, blocks, scheme, n),
                }
                for path, call in paths.items():
                    for group, sizes in SHAPE_GROUPS:
                        most = max(sizes)
                        against = run(call, x[:most])
                        if not raced:
                            rows[f"{scheme} {site} {path} {group} {most} repeat"] = row(run(call, x[:most]), against)
                        for m in sizes[:-1]:
                            rows[f"{scheme} {site} {path} {group} {m}"] = row(run(call, x[:m]), against[:m])
                del blocks, x
        torch.cuda.synchronize()
    return rows


@app.entrypoint(accelerator=True)
def batch_shapes(ctx: Context) -> Agreement:
    """cozy-runtime#2309's red-to-green: batch-shapes as it first ran, raced on torch's default stream with no sync, beside
    the same cases settled on the same card. The ggml path orders itself on the default stream where the raced rows equal
    the settled ones; told, with every repeat (a call against itself) identical as the control."""
    raced, settled = _shapes(raced=True), _shapes(raced=False)
    repeats = {k: v for k, v in settled.items() if k.endswith(" repeat")}
    differs = [k for k, v in raced.items() if settled.get(k) != v]
    return Agreement(json.dumps({
        "rows": len(raced), "raced_equals_settled": not differs, "differs": differs[:8],
        "repeat_identical": all(v["max_abs_diff"] == 0.0 for v in repeats.values()),
        "raced": raced, "settled": settled,
    }))  # fmt: skip


@app.entrypoint(accelerator=True)
def opening_kernel(ctx: Context, model: VisionLM) -> Agreement:
    """A decode whose first steps meet a kernel still opening on its card completes: a probe that takes 20 s is primed
    first (`kernel_ready.prime`), then `PROMPT` is decoded greedily for 32 tokens (xs-213: the first step's warm-up once
    failed the call with `Building`). Told: its tokens, its wall time and whether the probe was still opening at the end."""
    from cozy_runtime.internal import kernel_ready

    name = f"xs213-slow-probe cuda:{torch.cuda.current_device()}"

    def slow(wait: bool) -> object:
        time.sleep(20)
        return object()

    kernel_ready.prime(name, slow)
    net = model.pipe.components["model"]
    ids = torch.tensor([model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]])
    seconds, made = _timed(net, ids, 32)
    return Agreement(json.dumps({"tokens": len(made), "s": round(seconds, 3), "still_opening": kernel_ready.opening(name)}))


def _timed(net: torch.nn.Module, ids: torch.Tensor, tokens: int) -> tuple[float, list[int]]:
    inputs = {"input_ids": ids.to(net.device), "attention_mask": torch.ones_like(ids).to(net.device)}
    torch.cuda.synchronize()
    began = time.perf_counter()
    out = lm.generate(net, inputs, max_new_tokens=tokens, stop=[], pad=0, temperature=0.0)
    torch.cuda.synchronize()
    return time.perf_counter() - began, out[0].tolist()


class _HeadLinears(TorchFunctionMode):
    """Each plain linear in the body whose weight has `vocab` rows (a plain output head's), by the rows it was given: that
    head's reads of its weight, in call order. A function mode, never a module hook: a hook on the model makes the engine
    refuse to record its step and stop drafting (`lm._capture_contract`: "custom Python hooks"). Only what Python runs
    shows; a replayed graph runs none."""

    def __init__(self, vocab: int) -> None:
        super().__init__()
        self.vocab = vocab
        self.rows: list[int] = []

    def __torch_function__(self, func, types, args=(), kwargs=None):  # type: ignore[no-untyped-def]  # torch's hook
        kwargs = kwargs or {}
        weight = args[1] if len(args) > 1 else kwargs.get("weight")
        if func is torch.nn.functional.linear and isinstance(weight, torch.Tensor) and weight.dim() == 2 and weight.shape[0] == self.vocab:
            self.rows.append(args[0].numel() // args[0].shape[-1])
        return func(*args, **kwargs)


def _head(net: torch.nn.Module) -> dict[str, object]:
    """The output head's module and the bytes it holds as parameters and buffers; told, never judged."""
    try:
        out = net.get_output_embeddings()
        held = sum(t.numel() * t.element_size() for t in (*out.parameters(), *out.buffers()))
        return {"module": type(out).__name__, "rows": int(out.out_features), "bytes": held}
    except Exception as e:  # noqa: BLE001  # a head of another shape tells what it is, and the timing goes on
        return {"unread": f"{type(e).__name__}: {e}"[:200]}


@app.entrypoint(accelerator=True)
def decode(ctx: Context, model: VisionLM) -> Agreement:
    """`PROMPT` decoded greedily for 512 tokens, never stopping, with the MTP head drafting and without: wall tokens/s
    (its ~40-token prefill included), each after a warm-up of the same length, so the timed call captures nothing (the
    windows a 512-token call grows into are captured by the warm-up), and whether both made the same tokens; `s` is the
    timed call's wall time. Beside them the output head's module and bytes."""
    net = model.pipe.components["model"]
    ids = torch.tensor([model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]])
    head = lm.HEADS.get(net)
    rows: dict[str, object] = {"prompt_tokens": int(ids.shape[1]), "gpu": torch.cuda.get_device_name()}
    rows["head"] = _head(net)
    made: dict[str, list[int]] = {}
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del lm.HEADS[net]
        try:
            _timed(net, ids, 512)
            seconds, made[mode] = _timed(net, ids, 512)
        finally:
            if head is not None:
                lm.HEADS[net] = head
        rows[mode] = {"tokens": len(made[mode]), "s": round(seconds, 3), "tok_s": round(len(made[mode]) / seconds, 1)}
    if len(made) == 2:
        a, b = made["drafted"], made["plain"]
        rows["same_tokens"] = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
    return Agreement(json.dumps(rows))


@app.entrypoint(accelerator=True)
def head_reads(ctx: Context, model: VisionLM) -> Agreement:
    """How often a decode step reads a plain output head's weight, with the MTP head drafting and without: `PROMPT`
    decoded for 12 tokens, sampled, so its steps are new to an engine that has recorded the greedy ones and Python runs
    each once before it replays (the prefill, the first step, and the step as it is recorded). `head_linears`: the rows
    of each plain linear on the head's weight, in call order (`_HeadLinears`); an encoded head shows none, its kernel
    takes a call's rows together. Never timed: run after the timed calls."""
    net = model.pipe.components["model"]
    ids = torch.tensor([model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]])
    inputs = {"input_ids": ids.to(net.device), "attention_mask": torch.ones_like(ids).to(net.device)}
    head = lm.HEADS.get(net)
    rows: dict[str, object] = {"prompt_tokens": int(ids.shape[1]), "head": _head(net)}
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del lm.HEADS[net]
        try:
            with _HeadLinears(int(net.get_output_embeddings().out_features)) as seen:  # type: ignore[operator]  # a causal LM's
                made = lm.generate(net, inputs, max_new_tokens=12, stop=[], pad=0, temperature=0.7, seed=213)
        finally:
            if head is not None:
                lm.HEADS[net] = head
        rows[mode] = {"tokens": int(made.shape[1]), "head_linears": seen.rows}
    return Agreement(json.dumps(rows))
