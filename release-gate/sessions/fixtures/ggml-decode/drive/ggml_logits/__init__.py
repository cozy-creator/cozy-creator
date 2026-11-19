"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up).
- `quality`: one lane's top-1 next tokens and perplexity on `TEXTS`, compared across calls.
- `opening_kernel`: cr-342's cell through the Runtime's bench (cozy-runtime#2322), in this environment, which holds
  the Transformers it builds its small model with."""

import json
import time
from collections.abc import Callable
from typing import cast

import msgspec
import torch
from cozy_runtime import bench, lm
from cozy_runtime.author import App, Context, call
from cozy_runtime.vlm import VisionLM
from torch.overrides import TorchFunctionMode
from transformers import PreTrainedModel

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


@app.entrypoint(accelerator=False)
async def opening_kernel(ctx: Context) -> Agreement:
    """A cold decode whose first steps meet a kernel still building completes (`bench.lm_opening_kernel`, cr-342)."""
    return Agreement((await call(bench.lm_opening_kernel, bench.EmptyRequest())).detail)


def _timed(net: PreTrainedModel, ids: torch.Tensor, tokens: int) -> tuple[float, list[int]]:
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


def _output_head(net: PreTrainedModel) -> torch.nn.Module:
    """`net`'s output head (Transformers' `get_output_embeddings`, which it leaves unannotated)."""
    head = cast("Callable[[], torch.nn.Module | None]", net.get_output_embeddings)()
    if head is None:
        raise ValueError(f"{type(net).__name__} has no output head")
    return head


def _head(net: PreTrainedModel) -> dict[str, object]:
    """The output head's module and the bytes it holds as parameters and buffers; told, never judged."""
    try:
        out = _output_head(net)
        held = sum(t.numel() * t.element_size() for t in (*out.parameters(), *out.buffers()))
        return {"module": type(out).__name__, "rows": int(getattr(out, "out_features")), "bytes": held}
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
            with _HeadLinears(int(getattr(_output_head(net), "out_features"))) as seen:
                made = lm.generate(net, inputs, max_new_tokens=12, stop=[], pad=0, temperature=0.7, seed=213)
        finally:
            if head is not None:
                lm.HEADS[net] = head
        rows[mode] = {"tokens": int(made.shape[1]), "head_linears": seen.rows}
    return Agreement(json.dumps(rows))
