"""ggml-decode's logits check (xs-213): a lane's next-token distributions against a reference lane's, on the same text,
through the Runtime as a run loads either (its leaves, its kernels). Per position: whether the top tokens agree, and
KL(reference || lane) in nats."""

import json

import msgspec
import torch
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
