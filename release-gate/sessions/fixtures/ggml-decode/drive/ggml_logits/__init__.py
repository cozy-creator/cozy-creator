"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up)."""

import json
import time
from collections import Counter
from collections.abc import Iterator
from contextlib import contextmanager

import msgspec
import torch
from cozy_runtime import lm
from cozy_runtime.author import App, Context
from cozy_runtime.vlm import VisionLM

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


def _timed(net: torch.nn.Module, ids: torch.Tensor, tokens: int) -> tuple[float, list[int]]:
    inputs = {"input_ids": ids.to(net.device), "attention_mask": torch.ones_like(ids).to(net.device)}
    torch.cuda.synchronize()
    began = time.perf_counter()
    out = lm.generate(net, inputs, max_new_tokens=tokens, stop=[], pad=0, temperature=0.0)
    torch.cuda.synchronize()
    return time.perf_counter() - began, out[0].tolist()


@contextmanager
def _head_reads(net: torch.nn.Module) -> Iterator[list[list[int]]]:
    """The output head's reads in the body, by pass of the decoder: [the pass's positions, the head's calls after it, the
    rows they carried]. Only what Python runs shows: the prefill, and each step once as its graph is recorded (a replayed
    step runs none), which is one step's reads. A plain head reads its weight once a row of a verify pass
    (`lm._RowByRow`); a leaf's kernel takes a call's rows together."""
    passes: list[list[int]] = []

    def passed(_module: torch.nn.Module, _args: object, out: tuple[torch.Tensor, ...]) -> None:
        passes.append([int(out[0].shape[-2]), 0, 0])

    def read(_module: torch.nn.Module, args: tuple[torch.Tensor, ...]) -> None:
        if passes and args and isinstance(args[0], torch.Tensor):
            passes[-1][1] += 1
            passes[-1][2] += args[0].numel() // args[0].shape[-1]

    handles = (
        net.get_decoder().register_forward_hook(passed),
        net.get_output_embeddings().register_forward_pre_hook(read),
    )
    try:
        yield passes
    finally:
        for handle in handles:
            handle.remove()


@app.entrypoint(accelerator=True)
def decode(ctx: Context, model: VisionLM) -> Agreement:
    """`PROMPT` decoded greedily for 512 tokens, never stopping, with the MTP head drafting and without: wall tokens/s
    (its ~40-token prefill included), each after a warm-up of the same length, so the timed call captures nothing (the
    windows a 512-token call grows into are captured by the warm-up), and whether both made the same tokens. Beside each:
    the output head's bytes and its reads a pass, counted in the warm-up (`_head_reads`)."""
    net = model.pipe.components["model"]
    ids = torch.tensor([model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]])
    head = lm.HEADS.get(net)
    rows: dict[str, object] = {"prompt_tokens": int(ids.shape[1]), "gpu": torch.cuda.get_device_name()}
    out = net.get_output_embeddings()
    rows["head"] = {
        "module": type(out).__name__,
        "bytes": sum(t.numel() * t.element_size() for t in (*out.parameters(), *out.buffers())),
    }
    made: dict[str, list[int]] = {}
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del lm.HEADS[net]
        try:
            with _head_reads(net) as passes:
                _timed(net, ids, 512)
            seconds, made[mode] = _timed(net, ids, 512)
        finally:
            if head is not None:
                lm.HEADS[net] = head
        # [positions, head calls, head rows, passes like it], the warm-up's: its prefill and each recorded step.
        recorded = [[*key, n] for key, n in sorted(Counter(map(tuple, passes)).items())]
        rows[mode] = {"tokens": len(made[mode]), "s": round(seconds, 3), "tok_s": round(len(made[mode]) / seconds, 1),
                      "head_reads": recorded}
    if len(made) == 2:
        a, b = made["drafted"], made["plain"]
        rows["same_tokens"] = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
    return Agreement(json.dumps(rows))
