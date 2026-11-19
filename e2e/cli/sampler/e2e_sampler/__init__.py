"""cr-346's cell: `lm.generate` picks each token as vLLM 0.19.1 does. One real decode per setting on the
checkpoint the cell names, the logits the sampler saw at each step read off the LM head; at each step vLLM's own
functions (copied below from v0.19.1, Apache-2.0) take those logits through its order (penalties, temperature, min-p,
top-k and top-p), and the Runtime's token must be the Gumbel-max draw over what they keep with the Runtime's noise for
that seed and position. The RC's order (top-k before the presence penalty) is scored on the same logits beside it."""

from __future__ import annotations

import json
import math
import time
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from importlib import metadata
from typing import TypedDict

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
PROMPT = "List twenty fruits, one per line, each as 'Fruit: <name>, colour: <colour>'."
#: Free prose, long: many steps where several tokens are about as likely, the steps a penalty reorders.
STORY = "Write a long, rambling story about an old fisherman, his small boat and the sea, in plain words."
#: `order`'s calls: (name, prompt, settings, seed); each claim on its own, so a red says which failed.
CALLS: list[tuple[str, str, dict[str, float], int]] = [
    ("recipe-7", PROMPT, RECIPE, 7), ("recipe-8", PROMPT, RECIPE, 8),
    *((f"order-{seed}", STORY, ORDER, seed) for seed in (1, 2, 3, 4)),
    ("knobs", PROMPT, KNOBS, 7),
]
#: `drafted`'s settings: greedy without and with a penalty, then each of `order`'s.
DRAFTED: list[tuple[str, dict[str, float]]] = [
    ("greedy", {"temperature": 0.0}), ("greedy-presence", {"temperature": 0.0, "presence_penalty": 1.5}),
    ("recipe", RECIPE), ("order", ORDER), ("knobs", KNOBS),
]
#: A split between drafted and undrafted at a margin at least this wide (logits; z + Gumbel when sampled) is a clear choice
#: decided two ways, a bug; a narrower one is a near tie the verify pass's rounding may turn (the coordinator, 11-19).
CLEAR = 0.5



class Order(msgspec.Struct):
    max_tokens: int = 160


class Call(msgspec.Struct):
    name: str
    prompt: str
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


def get_token_bin_counts_and_mask(tokens: torch.Tensor, vocab_size: int,
                                  num_seqs: int) -> tuple[torch.Tensor, torch.Tensor]:
    bin_counts = torch.zeros((num_seqs, vocab_size + 1), dtype=torch.long, device=tokens.device)
    bin_counts.scatter_add_(1, tokens, torch.ones_like(tokens))
    bin_counts = bin_counts[:, :vocab_size]
    mask = bin_counts > 0
    return bin_counts, mask


def apply_repetition_penalties_torch(logits: torch.Tensor, prompt_mask: torch.Tensor, output_mask: torch.Tensor,
                                     repetition_penalties: torch.Tensor) -> None:
    repetition_penalties = repetition_penalties.unsqueeze(dim=1).repeat(1, logits.size(1))
    penalties = torch.where(prompt_mask | output_mask, repetition_penalties, 1.0)
    scaling = torch.where(logits > 0, 1.0 / penalties, penalties)
    logits *= scaling


def apply_penalties(logits: torch.Tensor, prompt_tokens_tensor: torch.Tensor, output_tokens_tensor: torch.Tensor,
                    presence_penalties: torch.Tensor, frequency_penalties: torch.Tensor,
                    repetition_penalties: torch.Tensor) -> torch.Tensor:
    num_seqs, vocab_size = logits.shape
    _, prompt_mask = get_token_bin_counts_and_mask(prompt_tokens_tensor, vocab_size, num_seqs)
    output_bin_counts, output_mask = get_token_bin_counts_and_mask(output_tokens_tensor, vocab_size, num_seqs)
    apply_repetition_penalties_torch(logits, prompt_mask, output_mask, repetition_penalties)
    logits -= frequency_penalties.unsqueeze(dim=1) * output_bin_counts
    logits -= presence_penalties.unsqueeze(dim=1) * output_mask
    return logits


def apply_temperature(logits: torch.Tensor, temp: torch.Tensor) -> torch.Tensor:
    return logits.div_(temp.unsqueeze(dim=1))


def min_p_apply(logits: torch.Tensor, min_p: torch.Tensor) -> torch.Tensor:
    probability_values = torch.nn.functional.softmax(logits, dim=-1)
    max_probabilities = torch.amax(probability_values, dim=-1, keepdim=True)
    adjusted_min_p = max_probabilities.mul_(min_p)
    invalid_token_mask = probability_values < adjusted_min_p
    logits.masked_fill_(invalid_token_mask, -float("inf"))
    return logits


def apply_top_k_top_p_pytorch(logits: torch.Tensor, k: torch.Tensor | None, p: torch.Tensor | None) -> torch.Tensor:
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


def _one(value: float, dtype: torch.dtype = torch.float32) -> torch.Tensor:
    """A one-row tensor of `value`: a batch of one, as vLLM's and the Runtime's samplers take each setting."""
    return torch.tensor([value], dtype=dtype)


class Knobs(TypedDict, total=False):
    """`lm.generate`'s sampling keywords."""

    temperature: float
    top_p: float
    top_k: int
    min_p: float
    presence_penalty: float
    frequency_penalty: float
    repetition_penalty: float


def knobs(s: dict[str, float]) -> Knobs:
    """`s` as `lm.generate`'s keywords, only those it names: a Runtime that takes fewer refuses the rest (TypeError)."""
    made = Knobs(temperature=s["temperature"])
    if "top_p" in s:
        made["top_p"] = s["top_p"]
    if "top_k" in s:
        made["top_k"] = int(s["top_k"])
    if "min_p" in s:
        made["min_p"] = s["min_p"]
    if "presence_penalty" in s:
        made["presence_penalty"] = s["presence_penalty"]
    if "frequency_penalty" in s:
        made["frequency_penalty"] = s["frequency_penalty"]
    if "repetition_penalty" in s:
        made["repetition_penalty"] = s["repetition_penalty"]
    return made


def _chat(model: VisionLM, text: str) -> str:
    """`text` as the model's chat template frames one user turn, thinking off."""
    chat = model.processor.apply_chat_template([{"role": "user", "content": text}], tokenize=False,
                                               add_generation_prompt=True, enable_thinking=False)
    assert isinstance(chat, str)
    return chat


def _device(hf: torch.nn.Module) -> torch.device:
    return next(hf.parameters()).device


def vllm_order(logits: torch.Tensor, prompt: torch.Tensor, output: list[int], s: dict[str, float]) -> torch.Tensor:
    """`logits` (`[1, vocab]`) as vLLM's `Sampler` leaves them to draw from (sampler.py:299 penalties, :177
    temperature, :183-184 min-p, :187 top-k and top-p); -inf outside what it keeps."""
    vocab, one = int(logits.shape[-1]), _one
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
    vocab, one = int(logits.shape[-1]), _one
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
    tokenizer = processor.tokenizer
    tokenizer.padding_side = "left"
    hf = model.pipe.components[vlm.COMPONENT]
    stops = [t for t in (tokenizer.convert_tokens_to_ids("<|im_end|>"), tokenizer.eos_token_id) if t is not None]
    head = hf.get_output_embeddings()  # type: ignore[no-untyped-call]  # unannotated in Transformers
    calls: list[Call] = []
    for name, text, settings, seed in CALLS:
        inputs = processor(text=[_chat(model, text)], return_tensors="pt").to(_device(hf))
        prompt = inputs["input_ids"][0].cpu()
        seen: list[torch.Tensor] = []
        handle = head.register_forward_hook(lambda _m, _a, out: seen.append(out[0, -1].float().cpu().clone()))
        try:
            made = lm.generate(hf, inputs, max_new_tokens=payload.max_tokens, stop=stops, pad=stops[0],
                               seed=seed, **knobs(settings))
        except TypeError as exc:  # a Runtime that takes fewer settings
            calls.append(Call(name, text[:40], seed, settings, 0, -1, -1, -1, -1, -1, 0.0, "",
                              error=f"{type(exc).__name__}: {exc}"))
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
            name, text[:40], seed, settings, len(tokens), mismatches, first, kept_equal if has_runtime else -1,
            old_kept if rc else -1, old_draw if rc else -1, kept / max(len(tokens), 1),
            str(tokenizer.decode(tokens, skip_special_tokens=True)),
        ))  # fmt: skip
    return Ordered(calls)


#: Qwen3.5-9B whole, its multi-token-prediction layer kept (the draft head `vlm.construct` registers), for `drafted`;
#: `qwen/qwen3.5-9b-mtp` is that layer alone, a drafter for another model.
DRAFTING = "qwen/qwen3.5-9b@0.2.0"


class Drafted(msgspec.Struct):
    #: which of `DRAFTED`'s settings
    setting: str
    #: decode with the model's draft head, or with it set aside (a fresh engine either way)
    drafting: bool = True
    #: a reference call's tokens (the first undrafted one's), which this call is judged against
    against: list[int] = []
    prompt: str = PROMPT
    max_tokens: int = 256
    seed: int = 7
    #: a label for the arm (its Runtime), so a second arm's call is no repeat of the first's
    arm: str = ""


class Decoded(msgspec.Struct):
    setting: str
    drafting: bool
    #: the checkpoint has a draft head; the Runtime that ran it
    head: bool
    runtime: str
    tokens: list[int]
    text: str
    #: against `against`: the same tokens, or where they split and by what margin there: the undrafted token's lead over
    #: the drafted one's, from a teacher-forced eager forward over the shared prefix through vLLM's own penalties (z + Gumbel
    #: when sampled), and which of them that forward picks
    same: bool = True
    first_difference: int = -1
    margin: float = 0.0
    teacher_picks: str = ""
    parts: dict[str, float] = {}
    error: str = ""


def split_margin(hf: torch.nn.Module, prompt: torch.Tensor, prefix: list[int], a: int, b: int, settings: dict[str, float],
                 seed: int) -> tuple[float, str, dict[str, float]]:
    """How near a tie tokens `a` (the reference call's) and `b` (the judged call's) are after `prefix`: a fresh eager forward
    over the prompt and prefix, its last logits through vLLM's penalties, and the smallest logit change that would turn one
    choice into the other, each part in logits: the draw's gap (the logits' gap greedy; T times the gap in z + Gumbel when
    sampled), and each token's distance from the top-k, min-p and top-p edges that decide whether it may be drawn at all (top-p
    to first order: the mass before it against top-p, over its own probability). Returns the least part (signed as the draw's
    gap), which token that forward picks, and the parts."""
    ids = torch.cat([prompt, torch.tensor(prefix, dtype=torch.long)])[None].to(_device(hf))
    with torch.inference_mode():
        logits = hf(input_ids=ids, attention_mask=torch.ones_like(ids), use_cache=False).logits[:, -1].float().cpu()
    vocab = int(logits.shape[-1])
    one = _one
    made = torch.tensor([prefix or [vocab]], dtype=torch.long)
    p, f, r = (settings.get(k, d) for k, d in (("presence_penalty", 0.0), ("frequency_penalty", 0.0), ("repetition_penalty", 1.0)))
    scored = apply_penalties(logits.clone(), prompt[None], made, one(p), one(f), one(r))[0]
    t = settings.get("temperature", 0.0)
    if t <= 0:
        pick = int(scored.argmax())
        gap = float(scored[a] - scored[b])
        parts = {"draw": gap}
    else:
        kept = vllm_order(logits, prompt, prefix, settings)[0]
        drawn = kept + gumbel(seed, len(prefix), vocab)[0]
        pick = int(drawn.argmax())
        noisy = scored / t + gumbel(seed, len(prefix), vocab)[0]
        gap = float(t * (noisy[a] - noisy[b]))
        parts = {"draw": gap}
        ordered = scored.sort(descending=True).values
        k = int(settings.get("top_k", 0))
        probabilities = (scored / t).softmax(-1)
        for name, x in (("a", a), ("b", b)):
            if 0 < k < vocab:
                parts[f"top_k_{name}"] = float(scored[x] - ordered[k] if scored[x] >= ordered[k - 1] else ordered[k - 1] - scored[x])
            if settings.get("min_p", 0.0) > 0:
                parts[f"min_p_{name}"] = float((scored[x] - (ordered[0] + t * math.log(settings["min_p"]))).abs())
            if settings.get("top_p", 1.0) < 1:
                above = float(probabilities[scored > scored[x]].sum())
                px = float(probabilities[x])
                if above > 0 and px > 0:
                    parts[f"top_p_{name}"] = t * abs(above - settings["top_p"]) / (above * px)
    least = min(abs(v) for v in parts.values())
    said = "reference" if pick == a else "judged" if pick == b else f"neither ({pick})"
    return (least if gap >= 0 else -least), said, parts


@app.entrypoint(defaults={"model": DRAFTING})
def drafted(payload: Drafted, model: VisionLM) -> Decoded:
    """One of `DRAFTED`'s settings decoded at one seed on a fresh engine, with the model's draft head or with it set aside;
    a drafted call given the undrafted one's tokens says where they split and the margin there (`CLEAR`). Its run's decode
    note says whether it drafted, and on how many recorded graphs."""
    settings = dict(DRAFTED)[payload.setting]
    processor = model.processor
    tokenizer = processor.tokenizer
    tokenizer.padding_side = "left"
    hf = model.pipe.components[vlm.COMPONENT]
    inputs = processor(text=[_chat(model, payload.prompt)], return_tensors="pt").to(_device(hf))
    prompt = inputs["input_ids"][0].cpu()
    stops = [t for t in (tokenizer.convert_tokens_to_ids("<|im_end|>"), tokenizer.eos_token_id) if t is not None]
    head = lm.HEADS.get(hf)
    runtime = metadata.version("cozy-runtime")
    lm.release(hf)  # a fresh engine: the head it drafts with, or none, and its own graphs
    if not payload.drafting:
        lm.HEADS.pop(hf, None)
    try:
        made = lm.generate(hf, inputs, max_new_tokens=payload.max_tokens, stop=stops, pad=stops[0], seed=payload.seed,
                           **knobs(settings))[0].tolist()
    except TypeError as exc:  # a Runtime that takes fewer settings
        return Decoded(payload.setting, payload.drafting, head is not None, runtime, [], "", error=f"{type(exc).__name__}: {exc}")
    finally:
        lm.release(hf)
        if head is not None:
            lm.HEADS[hf] = head
    end = next((i for i, t in enumerate(made) if t in stops), len(made) - 1)
    tokens = made[: end + 1]
    out = Decoded(payload.setting, payload.drafting, head is not None, runtime, tokens,
                  str(tokenizer.decode(tokens, skip_special_tokens=True)))
    if payload.against:
        plain = list(payload.against)
        first = next((i for i, (a, b) in enumerate(zip(plain, tokens, strict=False)) if a != b), -1)
        out.same = plain == tokens
        if first >= 0:
            out.first_difference = first
            out.margin, out.teacher_picks, out.parts = split_margin(hf, prompt, plain[:first], plain[first], tokens[first],
                                                                    settings, payload.seed)
        elif not out.same:
            out.first_difference = min(len(plain), len(tokens))
    return out


class Repeat(msgspec.Struct):
    prompt: str = STORY
    arm: str = ""


class Repeated(msgspec.Struct):
    #: the last position's logits, largest difference between two identical forwards
    logits_max_diff: float
    #: the first module, in the order the forward ran them, whose output differs between the two, and by how much
    first_module: str
    first_diff: float
    #: of the modules watched (every module of the first two decoder layers, each later layer's own children)
    differing: int
    watched: int
    runtime: str


@app.entrypoint(defaults={"model": DRAFTING})
def repeat(payload: Repeat, model: VisionLM) -> Repeated:
    """The same prompt through the model's own forward twice, eagerly: where their outputs first differ, the module whose
    kernels make a forward not reproducible run to run."""
    processor = model.processor
    hf = model.pipe.components[vlm.COMPONENT]
    ids = processor(text=[_chat(model, payload.prompt)], return_tensors="pt").to(_device(hf))["input_ids"]
    layers = hf.get_decoder().layers  # type: ignore[no-untyped-call]  # unannotated in Transformers
    watched: list[tuple[str, torch.nn.Module]] = []
    for i, layer in enumerate(layers):
        mods = layer.named_modules() if i < 2 else layer.named_children()
        watched += [(f"layers.{i}.{n}" if n else f"layers.{i}", m) for n, m in mods]
    runs: list[dict[str, torch.Tensor]] = [{}, {}]
    order: list[str] = []
    current = [0]

    def hook(name: str):  # type: ignore[no-untyped-def]
        def keep(_m, _a, out) -> None:  # type: ignore[no-untyped-def]
            t = out[0] if isinstance(out, tuple) else out
            if isinstance(t, torch.Tensor) and name not in runs[current[0]]:
                runs[current[0]][name] = t.detach().float().cpu()
                if current[0] == 0:
                    order.append(name)
        return keep

    handles = [m.register_forward_hook(hook(n)) for n, m in watched]
    lasts: list[torch.Tensor] = []
    try:
        for run in (0, 1):
            current[0] = run
            with torch.inference_mode():
                lasts.append(hf(input_ids=ids, attention_mask=torch.ones_like(ids), use_cache=False).logits[0, -1].float().cpu())
    finally:
        for h in handles:
            h.remove()
    diffs = {n: float((runs[0][n] - runs[1][n]).abs().max()) for n in order if n in runs[1] and runs[0][n].shape == runs[1][n].shape}
    first = next((n for n in order if diffs.get(n, 0.0) > 0), "")
    return Repeated(float((lasts[0] - lasts[1]).abs().max()), first, diffs.get(first, 0.0),
                    sum(1 for v in diffs.values() if v > 0), len(order), metadata.version("cozy-runtime"))


# ---- cr-351: a constraint that starts after the thought, and a conversation


#: `think_then_schema`'s seeds (llm-sampling's) and `free_line_then_schema`'s, one call each: a single draw could parse by
#: chance.
THOUGHT_SEEDS = (7, 8, 9)
#: Qwen3.5's card for thinking: up to 32,768 new tokens, at its general thinking profile (what Ask runs: `vlm.CARD[True]`,
#: top_k 20, presence 1.5).
THOUGHT_TOKENS = 32768


class _Timed:
    """A constraint's own time: what `then` spends answering, summed (a draft's checks included)."""

    def __init__(self, then: lm.Constraint) -> None:
        self.then, self.seconds = then, 0.0

    def allowed(self, tokens: Sequence[int]) -> Sequence[int] | None:
        start = time.perf_counter()
        try:
            return self.then.allowed(tokens)
        finally:
            self.seconds += time.perf_counter() - start


@contextmanager
def _drafting(hf: torch.nn.Module, on: bool) -> Iterator[None]:
    """A fresh engine for the calls inside, with the checkpoint's draft head or without it; the head back after."""
    head = lm.HEADS.get(hf)
    lm.release(hf)
    if not on:
        lm.HEADS.pop(hf, None)
    try:
        yield
    finally:
        lm.release(hf)
        if head is not None:
            lm.HEADS[hf] = head


class Fruit(msgspec.Struct, forbid_unknown_fields=True):
    name: str
    seeds: int


class Fruits(msgspec.Struct, forbid_unknown_fields=True):
    fruits: list[Fruit]


#: Each answer's JSON schema, as outlines compiles it (`Fruit` and `Fruits`, which parse it back).
FRUIT = {"type": "object", "properties": {"name": {"type": "string"}, "seeds": {"type": "integer"}},
         "required": ["name", "seeds"], "additionalProperties": False}
SCHEMAS: dict[type[msgspec.Struct], dict[str, object]] = {
    Fruit: FRUIT,
    Fruits: {"type": "object", "properties": {"fruits": {"type": "array", "items": FRUIT}}, "required": ["fruits"],
             "additionalProperties": False},
}


def _index(model: VisionLM, schema: type[msgspec.Struct]) -> lm.Index:
    """outlines' index of `schema` over the model's tokenizer (an outlines-core `Index`)."""
    from outlines import from_transformers
    from outlines.generator import SteerableGenerator
    from outlines.types import JsonSchema

    made = SteerableGenerator(from_transformers(model.pipe.components[vlm.COMPONENT], model.processor.tokenizer),
                              JsonSchema(json.dumps(SCHEMAS[schema])))
    assert made.logits_processor is not None
    index: lm.Index = made.logits_processor.index
    return index


def _parses(schema: type[msgspec.Struct], text: str) -> bool:
    try:
        msgspec.json.decode(text, type=schema)
    except msgspec.DecodeError:  # not JSON, or not the schema (ValidationError)
        return False
    return True


class Thought(msgspec.Struct):
    prompt: str = "Name one fruit and how many seeds it usually has."
    max_tokens: int = THOUGHT_TOKENS
    #: one call a run (llm-sampling takes `THOUGHT_SEEDS` across its arms, each arm's first seed before any second)
    seed: int = THOUGHT_SEEDS[0]
    #: the constraint's cost (llm-sampling's `think` part): without it, and without the draft head
    constrained: bool = True
    drafting: bool = True
    arm: str = ""


class Thinking(msgspec.Struct):
    #: an answer followed the thought within `max_tokens` (it closed), and it parses exactly to the schema
    closed: bool
    parsed: bool
    thought_tokens: int
    answer_tokens: int
    seconds: float
    #: the grammar's own time (its `Index` steps), per answer token; 0 without the constraint
    grammar_ms: float
    thought_tail: str
    answer: str
    runtime: str
    error: str = ""


@app.entrypoint(defaults={"model": DRAFTING})
def think_then_schema(payload: Thought, model: VisionLM) -> Thinking:
    """Qwen3.5-9B thinks freely, then answers held to a JSON schema (`Params.allowed`, an `lm.Grammar` over outlines'
    index of the schema, which the engine starts after the thought with `lm.After`), as its card sets it: the general
    thinking profile and up to 32,768 new tokens. The engine's own call, which `VisionLM.ask` answers from, so the run
    says whether its thought closed and how many tokens each part took."""
    runtime = metadata.version("cozy-runtime")
    hf = model.pipe.components[vlm.COMPONENT]
    tokenizer = model.processor.tokenizer
    try:
        grammar = lm.Grammar
        timed = _Timed(grammar(_index(model, Fruit))) if payload.constrained else None
        with _drafting(hf, payload.drafting):
            start = time.perf_counter()
            final = model.engine.generate(
                [vlm.Question(payload.prompt)],
                vlm.Params(system="", thinking=True, reasoning_effort=None, stop=[], max_new_tokens=payload.max_tokens,
                           temperature=1.0, top_p=0.95, top_k=20, presence_penalty=1.5, seed=payload.seed, allowed=timed),
                on_delta=lambda delta: None,
            )
            seconds = time.perf_counter() - start
    except (TypeError, AttributeError) as exc:  # a Runtime without `allowed` after the thought
        return Thinking(False, False, 0, 0, 0.0, 0.0, "", "", runtime, error=f"{type(exc).__name__}: {exc}")
    thought, answer = final.thoughts[0], final.texts[0].strip()
    told = len(tokenizer.encode(answer, add_special_tokens=False))
    return Thinking(
        bool(answer), bool(answer) and _parses(Fruit, answer), len(tokenizer.encode(thought, add_special_tokens=False)),
        told, round(seconds, 2), round(1000 * timed.seconds / max(told, 1), 3) if timed else 0.0, thought[-400:],
        answer[-400:], runtime,
    )


class Listing(msgspec.Struct):
    prompt: str = "List ten fruits and how many seeds each usually has, as JSON."
    max_tokens: int = 512
    constrained: bool = True
    drafting: bool = True
    arm: str = ""


class Listed(msgspec.Struct):
    answer: str
    parsed: bool
    tokens: int
    #: the grammar's own time (its `Index` steps), per token; 0 without the constraint
    grammar_ms: float
    runtime: str
    error: str = ""


@app.entrypoint(defaults={"model": DRAFTING})
def schema_cost(payload: Listing, model: VisionLM) -> Listed:
    """The JSON grammar's cost, which a thought's short answer is too brief to show: one answer without thinking, ten
    fruits held to their schema (`VisionLM.ask`'s `allowed`) or free, at Qwen's card profile without thinking
    (`vlm.CARD[False]`: T 0.7, top_p 0.8, top_k 20, presence 1.5). Its decode note gives ms/token."""
    runtime = metadata.version("cozy-runtime")
    hf = model.pipe.components[vlm.COMPONENT]
    tokenizer = model.processor.tokenizer
    try:
        grammar = lm.Grammar
        timed = _Timed(grammar(_index(model, Fruits))) if payload.constrained else None
        with _drafting(hf, payload.drafting):
            (answer,) = model.ask([vlm.Question(payload.prompt)], system="", thinking=False, stop=[],
                                  max_new_tokens=payload.max_tokens, temperature=0.7, top_p=0.8, top_k=20,
                                  presence_penalty=1.5, seed=7, allowed=timed)
    except (TypeError, AttributeError) as exc:  # a Runtime without `allowed`
        return Listed("", False, 0, 0.0, runtime, error=f"{type(exc).__name__}: {exc}")
    tokens = len(tokenizer.encode(answer, add_special_tokens=False))
    grammar_ms = round(1000 * timed.seconds / max(tokens, 1), 3) if timed else 0.0
    return Listed(answer[-600:], _parses(Fruits, answer), tokens, grammar_ms, runtime)


class Line(msgspec.Struct):
    prompt: str = ("Write one short sentence about a fruit on the first line. On the next line, give that fruit's name and "
                   "its usual number of seeds as JSON.")
    max_tokens: int = 160


class Lined(msgspec.Struct):
    #: at each seed (`THOUGHT_SEEDS`) `VisionLM.ask`'s answer: a free first line (prose, the constraint not yet started),
    #: then, held by `lm.After` on the newline, JSON that must parse exactly to the schema
    answers: list[str]
    free: list[bool]
    parsed: list[bool]
    runtime: str
    error: str = ""


@app.entrypoint(defaults={"model": MODEL})
def free_line_then_schema(payload: Line, model: VisionLM) -> Lined:
    """`think_then_schema`'s mechanism on a CPU budget: `VisionLM.ask`'s `allowed` as `lm.After` the first newline over an
    `lm.Grammar` of the schema (the thinking path wraps `After` on `</think>` itself, which the 0.8B never reaches in
    time). The precise profile (T 0.6, top_p 0.95, top_k 20)."""
    runtime = metadata.version("cozy-runtime")
    tokenizer = model.processor.tokenizer
    newline = list(tokenizer.encode("\n", add_special_tokens=False))
    try:
        after, grammar = lm.After, lm.Grammar
        index = _index(model, Fruit)
        texts = [model.ask([vlm.Question(payload.prompt)], system="", thinking=False, stop=[],
                           max_new_tokens=payload.max_tokens, temperature=0.6, top_p=0.95, top_k=20,
                           presence_penalty=0.0, seed=seed, allowed=after(newline, grammar(index)))[0]
                 for seed in THOUGHT_SEEDS]
    except (TypeError, AttributeError) as exc:  # a Runtime without `allowed` after a marker
        return Lined([], [], [], runtime, error=f"{type(exc).__name__}: {exc}")
    free, parsed = [], []
    for text in texts:
        line, _, rest = text.partition("\n")
        free.append(len(line.split()) >= 3 and not line.lstrip().startswith("{"))
        parsed.append(_parses(Fruit, rest.strip()))
    return Lined([t[:600] for t in texts], free, parsed, runtime)


class Remember(msgspec.Struct):
    name: str = "Zorblat"


class Remembered(msgspec.Struct):
    answer: str
    #: the answer names what only the earlier turns said
    recalled: bool
    runtime: str
    error: str = ""


@app.entrypoint(defaults={"model": MODEL})
def conversation(payload: Remember, model: VisionLM) -> Remembered:
    """A conversation through `VisionLM.ask`: a user turn and the assistant's reply before the question, which only
    they answer."""
    runtime = metadata.version("cozy-runtime")
    try:
        turn = vlm.Turn
        turns = [turn("user", [f"Hello, my name is {payload.name}."]), turn("assistant", [f"Nice to meet you, {payload.name}!"])]
        asked = vlm.Question("What is my name? Answer with the name only.", turns=turns)
    except (TypeError, AttributeError) as exc:  # a Runtime without conversations
        return Remembered("", False, runtime, error=f"{type(exc).__name__}: {exc}")
    (text,) = model.ask([asked], system="", thinking=False, stop=[], max_new_tokens=16, presence_penalty=0.0,
                        temperature=0.0, top_p=1.0, seed=0)
    return Remembered(text, payload.name.lower() in text.lower(), runtime)
