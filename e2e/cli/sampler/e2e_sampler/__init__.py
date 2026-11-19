"""cr-346's cell: `lm.generate` picks each token as vLLM 0.19.1 does. One real decode per setting on the
checkpoint the cell names, the logits the sampler saw at each step read off the LM head; at each step vLLM's own
functions (copied below from v0.19.1, Apache-2.0) take those logits through its order (penalties, temperature, min-p,
top-k and top-p), and the Runtime's token must be the Gumbel-max draw over what they keep with the Runtime's noise for
that seed and position. The RC's order (top-k before the presence penalty) is scored on the same logits beside it."""

from __future__ import annotations

from importlib import metadata

import msgspec
import torch
from cozy_runtime import lm, vlm
from cozy_runtime.author import App
from cozy_runtime.vlm import VisionLM

app = App()

MODEL = "huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0"
#: Qwen-Image-2.1's t2i enhancer (prompt_rewrite/pe_core.py PROFILES), which the RC's order ran wrong.
RECIPE = {"temperature": 1.0, "top_p": 0.95, "top_k": 20, "presence_penalty": 1.5}
#: A top-k that binds at every step (its nucleus is all of what top-k keeps), so the order changes the kept set wherever a
#: generated token is among the likeliest: the order's own case.
ORDER = {"temperature": 1.0, "top_p": 1.0, "top_k": 5, "presence_penalty": 1.5}
#: Every setting vLLM takes.
KNOBS = {"temperature": 0.8, "top_p": 0.9, "top_k": 40, "min_p": 0.05, "presence_penalty": 0.5, "frequency_penalty": 0.5,
         "repetition_penalty": 1.2}
#: `order`'s calls: (name, settings, seed); each claim on its own, so a red says which failed.
CALLS: list[tuple[str, dict[str, float], int]] = [
    ("recipe-7", RECIPE, 7), ("recipe-8", RECIPE, 8), ("order", ORDER, 7), ("knobs", KNOBS, 7),
]
#: `drafted`'s settings: greedy with a penalty, then each of `order`'s.
DRAFTED = [{"temperature": 0.0, "presence_penalty": 1.5}, RECIPE, ORDER, KNOBS]


PROMPT = "List twenty fruits, one per line, each as 'Fruit: <name>, colour: <colour>'."


class Order(msgspec.Struct):
    prompt: str = PROMPT
    max_tokens: int = 160


class Call(msgspec.Struct):
    name: str
    seed: int
    settings: dict[str, float]
    steps: int
    #: steps whose token is not vLLM's draw from the same logits and noise, and the first
    mismatches: int
    first_mismatch: int
    #: steps where the Runtime's own kept set (`cozy_runtime.sampling`) is vLLM's; -1: no such module (the RC)
    kept_equal: int
    #: steps where the RC's order keeps another set, and where it draws another token (-1: settings it never took)
    old_kept_differs: int
    old_draw_differs: int
    kept_mean: float
    text: str
    error: str = ""


class Ordered(msgspec.Struct):
    calls: list[Call]


# ---- vLLM v0.19.1, verbatim but for names: model_executor/layers/utils.py:34-89, _custom_ops.py:445-458,
# v1/sample/sampler.py:132-141, v1/sample/logits_processor/builtin.py:101-115, v1/sample/ops/topk_topp_sampler.py:258-299


def get_token_bin_counts_and_mask(tokens, vocab_size, num_seqs):  # type: ignore[no-untyped-def]
    bin_counts = torch.zeros((num_seqs, vocab_size + 1), dtype=torch.long, device=tokens.device)
    bin_counts.scatter_add_(1, tokens, torch.ones_like(tokens))
    bin_counts = bin_counts[:, :vocab_size]
    mask = bin_counts > 0
    return bin_counts, mask


def apply_repetition_penalties_torch(logits, prompt_mask, output_mask, repetition_penalties):  # type: ignore[no-untyped-def]
    repetition_penalties = repetition_penalties.unsqueeze(dim=1).repeat(1, logits.size(1))
    penalties = torch.where(prompt_mask | output_mask, repetition_penalties, 1.0)
    scaling = torch.where(logits > 0, 1.0 / penalties, penalties)
    logits *= scaling


def apply_penalties(logits, prompt_tokens_tensor, output_tokens_tensor, presence_penalties,  # type: ignore[no-untyped-def]
                    frequency_penalties, repetition_penalties):
    num_seqs, vocab_size = logits.shape
    _, prompt_mask = get_token_bin_counts_and_mask(prompt_tokens_tensor, vocab_size, num_seqs)
    output_bin_counts, output_mask = get_token_bin_counts_and_mask(output_tokens_tensor, vocab_size, num_seqs)
    apply_repetition_penalties_torch(logits, prompt_mask, output_mask, repetition_penalties)
    logits -= frequency_penalties.unsqueeze(dim=1) * output_bin_counts
    logits -= presence_penalties.unsqueeze(dim=1) * output_mask
    return logits


def apply_temperature(logits, temp):  # type: ignore[no-untyped-def]
    return logits.div_(temp.unsqueeze(dim=1))


def min_p_apply(logits, min_p):  # type: ignore[no-untyped-def]
    probability_values = torch.nn.functional.softmax(logits, dim=-1)
    max_probabilities = torch.amax(probability_values, dim=-1, keepdim=True)
    adjusted_min_p = max_probabilities.mul_(min_p)
    invalid_token_mask = probability_values < adjusted_min_p
    logits.masked_fill_(invalid_token_mask, -float("inf"))
    return logits


def apply_top_k_top_p_pytorch(logits, k, p):  # type: ignore[no-untyped-def]
    if p is None:
        if k is None:
            return logits
    logits_sort, logits_idx = logits.sort(dim=-1, descending=False)
    if k is not None:
        top_k_mask = logits_sort.size(1) - k.to(torch.long)
        top_k_mask = logits_sort.gather(1, top_k_mask.unsqueeze(dim=1))
        top_k_mask = logits_sort < top_k_mask
        logits_sort.masked_fill_(top_k_mask, -float("inf"))
    if p is not None:
        probs_sort = logits_sort.softmax(dim=-1)
        probs_sum = torch.cumsum(probs_sort, dim=-1, out=probs_sort)
        top_p_mask = probs_sum <= 1 - p.unsqueeze(dim=1)
        top_p_mask[:, -1] = False
        logits_sort.masked_fill_(top_p_mask, -float("inf"))
    return logits.scatter_(dim=-1, index=logits_idx, src=logits_sort)


# ---- end of vLLM's


def vllm_order(logits: torch.Tensor, prompt: torch.Tensor, output: list[int], s: dict[str, float]) -> torch.Tensor:
    """`logits` (`[1, vocab]`) as vLLM's `Sampler` leaves them to draw from (sampler.py:299 penalties, :177
    temperature, :183-184 min-p, :187 top-k and top-p); -inf outside what it keeps."""
    vocab, one = int(logits.shape[-1]), lambda v: torch.tensor([float(v)])
    out = logits.clone()
    presence, frequency = s.get("presence_penalty", 0.0), s.get("frequency_penalty", 0.0)
    repetition = s.get("repetition_penalty", 1.0)
    if presence or frequency or repetition != 1.0:
        made = torch.tensor([output or [vocab]], dtype=torch.long)  # vLLM pads with the vocabulary's size
        out = apply_penalties(out, prompt[None], made, one(presence), one(frequency), one(repetition))
    out = apply_temperature(out, one(s["temperature"]))
    if s.get("min_p", 0.0):
        out = min_p_apply(out, one(s["min_p"])[:, None])
    k = torch.tensor([int(s["top_k"])]) if s.get("top_k", 0) else None
    p = one(s["top_p"]) if s.get("top_p", 1.0) < 1 else None
    return apply_top_k_top_p_pytorch(out, k, p)


def rc_order(logits: torch.Tensor, output: list[int], s: dict[str, float]) -> torch.Tensor:
    """The RC's (cozy-runtime 24fb6ecc4 `_Engine._select`): top-k, then the presence penalty, then the temperature
    and top-p nucleus."""
    out = logits.clone()
    if s.get("top_k", 0):
        kth = out.topk(int(s["top_k"]), dim=-1).values[..., -1:]
        out = out.masked_fill(out < kth, float("-inf"))
    seen = torch.zeros_like(out)
    if output:
        seen[0, torch.tensor(output)] = 1.0
    z = (out - s.get("presence_penalty", 0.0) * seen) / s["temperature"]
    ordered, order = z.sort(-1, descending=True)
    probabilities = ordered.softmax(-1)
    cut = (probabilities.cumsum(-1) - probabilities >= s["top_p"]) & (s["top_p"] < 1)
    return z.masked_fill(torch.zeros_like(cut).scatter_(-1, order, cut), float("-inf"))


_LOW32 = 0xFFFFFFFF


def _times(x: torch.Tensor, constant: int) -> torch.Tensor:
    return (x * (constant & 0xFFFF) + (((x * (constant >> 16)) & 0xFFFF) << 16)) & _LOW32


def _mix(x: torch.Tensor) -> torch.Tensor:
    x = x ^ (x >> 16)
    x = _times(x, 0x7FEB352D)
    x = x ^ (x >> 15)
    x = _times(x, 0x846CA68B)
    return x ^ (x >> 16)


def gumbel(seed: int, index: int, vocab: int) -> torch.Tensor:
    """The Runtime's noise for output position `index` of a row seeded `seed` (`lm._draw`'s hash, the same on the RC
    and the branch), `[1, vocab]`."""
    s = torch.tensor([seed], dtype=torch.long)
    base = _mix(_mix(s & _LOW32) ^ ((s >> 32) & _LOW32))
    key = _mix(base ^ _mix(torch.tensor([index], dtype=torch.long) & _LOW32))
    bits = _mix(key[:, None] ^ _mix(torch.arange(vocab, dtype=torch.long)))
    uniform = (((bits >> 8).float() + 0.5) * (1.0 / (1 << 24))).clamp_(max=1 - 2**-24)
    return -torch.log(-torch.log(uniform))


def runtime_kept(logits: torch.Tensor, prompt: torch.Tensor, output: list[int], s: dict[str, float]) -> torch.Tensor | None:
    """What the Runtime's own sampler keeps from `logits` (`cozy_runtime.sampling`), None where it has none."""
    try:
        from cozy_runtime import sampling
    except ImportError:
        return None
    vocab, one = int(logits.shape[-1]), lambda v, d=torch.float32: torch.tensor([v], dtype=d)
    counts = torch.zeros((1, vocab))
    for token in output:
        counts[0, token] += 1
    prompted = sampling.prompted(prompt[None], torch.ones_like(prompt[None]), vocab)
    scores = sampling.penalized(logits.clone(), counts, prompted, one(s.get("presence_penalty", 0.0)),
                                one(s.get("frequency_penalty", 0.0)), one(s.get("repetition_penalty", 1.0)))
    return sampling.warped(scores, one(s["temperature"]), one(s.get("top_p", 1.0)),
                           one(int(s.get("top_k", 0)), torch.long), one(s.get("min_p", 0.0)))


@app.entrypoint(defaults={"model": MODEL})
def order(payload: Order, model: VisionLM) -> Ordered:
    """Each setting's decode, scored step by step against vLLM's sampler on the logits it saw."""
    processor = model.processor
    tokenizer = processor.tokenizer  # type: ignore[attr-defined]
    tokenizer.padding_side = "left"
    hf = model.pipe.components[vlm.COMPONENT]
    chat = processor.apply_chat_template(  # type: ignore[attr-defined]
        [{"role": "user", "content": [{"type": "text", "text": payload.prompt}]}],
        tokenize=False, add_generation_prompt=True, enable_thinking=False,
    )
    inputs = processor(text=[chat], return_tensors="pt").to(hf.device)  # type: ignore[operator]
    prompt = inputs["input_ids"][0].cpu()
    stops = [t for t in (tokenizer.convert_tokens_to_ids("<|im_end|>"), tokenizer.eos_token_id) if t is not None]
    head = hf.get_output_embeddings()
    calls: list[Call] = []
    for name, settings, seed in CALLS:
        seen: list[torch.Tensor] = []
        handle = head.register_forward_hook(lambda _m, _a, out: seen.append(out[0, -1].float().cpu().clone()))
        try:
            knobs = {k: int(v) if k == "top_k" else v for k, v in settings.items()}
            made = lm.generate(hf, inputs, max_new_tokens=payload.max_tokens, stop=stops, pad=stops[0],
                               seed=seed, **knobs)
        except TypeError as exc:  # a Runtime that takes fewer settings
            calls.append(Call(name, seed, settings, 0, -1, -1, -1, -1, -1, 0.0, "", error=f"{type(exc).__name__}: {exc}"))
            continue
        finally:
            handle.remove()
        tokens = made[0].tolist()
        end = next((i for i, t in enumerate(tokens) if t in stops), len(tokens) - 1)
        tokens = tokens[: end + 1]
        if len(seen) < len(tokens):
            raise RuntimeError(f"the head was read {len(seen)} times for {len(tokens)} tokens")
        mismatches, first, kept_equal, old_kept, old_draw, kept = 0, -1, 0, 0, 0, 0
        has_runtime = True
        rc = set(settings) <= {"temperature", "top_p", "top_k", "presence_penalty"}
        for t, token in enumerate(tokens):
            logits, output = seen[t][None], tokens[:t]
            want = vllm_order(logits, prompt, output, settings)
            noise = gumbel(seed, t, int(logits.shape[-1]))
            drawn = int((want + noise).argmax())
            kept += int(torch.isfinite(want).sum())
            if drawn != token:
                mismatches += 1
                first = t if first < 0 else first
            ours = runtime_kept(logits, prompt, output, settings)
            if ours is None:
                has_runtime = False
            elif torch.equal(torch.isfinite(ours), torch.isfinite(want)):
                kept_equal += 1
            if rc:
                old = rc_order(logits, output, settings)
                old_kept += int(not torch.equal(torch.isfinite(old), torch.isfinite(want)))
                old_draw += int(int((old + noise).argmax()) != drawn)
        calls.append(Call(
            name, seed, settings, len(tokens), mismatches, first, kept_equal if has_runtime else -1,
            old_kept if rc else -1, old_draw if rc else -1, kept / max(len(tokens), 1),
            str(tokenizer.decode(tokens, skip_special_tokens=True)),
        ))  # fmt: skip
    return Ordered(calls)


#: A checkpoint with a draft head (its multi-token-prediction layer), for `drafted`.
DRAFTING = "qwen/qwen3.5-9b-mtp@0.2.0"


class Drafted(msgspec.Struct):
    prompt: str = PROMPT
    max_tokens: int = 256
    seed: int = 7


class Pair(msgspec.Struct):
    settings: dict[str, float]
    plain: int
    drafted: int
    #: the drafted call's tokens are the undrafted one's
    same: bool
    first_difference: int
    plain_text: str
    drafted_text: str


class Pairs(msgspec.Struct):
    pairs: list[Pair]
    #: the checkpoint has a draft head
    head: bool
    #: the Runtime that ran it
    runtime: str


@app.entrypoint(defaults={"model": DRAFTING})
def drafted(payload: Drafted, model: VisionLM) -> Pairs:
    """Each setting decoded twice at one seed: without the model's draft head (a fresh engine, the head set aside), then
    with it. Their tokens must be the same; the run's notes say which call drafted and that its steps replayed graphs."""
    processor = model.processor
    tokenizer = processor.tokenizer  # type: ignore[attr-defined]
    tokenizer.padding_side = "left"
    hf = model.pipe.components[vlm.COMPONENT]
    chat = processor.apply_chat_template(  # type: ignore[attr-defined]
        [{"role": "user", "content": [{"type": "text", "text": payload.prompt}]}],
        tokenize=False, add_generation_prompt=True, enable_thinking=False,
    )
    inputs = processor(text=[chat], return_tensors="pt").to(hf.device)  # type: ignore[operator]
    stops = [t for t in (tokenizer.convert_tokens_to_ids("<|im_end|>"), tokenizer.eos_token_id) if t is not None]
    head = lm.HEADS.get(hf)

    def decode(settings: dict[str, float]) -> list[int]:
        knobs = {k: int(v) if k == "top_k" else v for k, v in settings.items()}
        made = lm.generate(hf, inputs, max_new_tokens=payload.max_tokens, stop=stops, pad=stops[0],
                           seed=payload.seed, **knobs)[0].tolist()
        end = next((i for i, t in enumerate(made) if t in stops), len(made) - 1)
        return made[: end + 1]

    pairs: list[Pair] = []
    for settings in DRAFTED:
        lm.release(hf)
        lm.HEADS.pop(hf, None)
        try:
            plain = decode(settings)
        finally:
            lm.release(hf)
            if head is not None:
                lm.HEADS[hf] = head
        drafts = decode(settings)
        first = next((i for i, (a, b) in enumerate(zip(plain, drafts, strict=False)) if a != b), -1)
        if first < 0 and len(plain) != len(drafts):
            first = min(len(plain), len(drafts))
        pairs.append(Pair(
            settings, len(plain), len(drafts), plain == drafts, first,
            str(tokenizer.decode(plain, skip_special_tokens=True)), str(tokenizer.decode(drafts, skip_special_tokens=True)),
        ))  # fmt: skip
    return Pairs(pairs, head is not None, metadata.version("cozy-runtime"))
