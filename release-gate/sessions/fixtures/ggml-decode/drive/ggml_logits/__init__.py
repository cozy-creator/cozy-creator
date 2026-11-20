"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up).
- `rows`, `routes`: the engine-routes session's calls that `qwen3/ask` can't make."""

import hashlib
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


#: A choice is clear where Transformers' one-pass forward and its own cached decode both lead by at least this.
CLEAR = 0.5


def _forced(net: torch.nn.Module, prompt: list[int], made: list[int],
            memo: dict[tuple[tuple[int, ...], tuple[int, ...]], torch.Tensor]) -> dict[str, object]:  # fmt: skip
    """`made` held against Transformers' teacher-forced reference: at each clear position, whether `made` took its
    choice. Clear: Transformers' forward over the prompt and `made` in one pass and its own cached decode of the same
    tokens, one at a time, both lead by at least `CLEAR` with the same top. A position only the forward leads at is
    not one the reference is sure of (44638784973: the forward led by 0.625 at token 434, its decode by 0.25), so a
    two-way bound replaces the forward's alone (`forward_clear`, `forward_agreed`: that old count). Misses by the old
    count (`misses`, the first 8) say both reads there; `split`, positions where both lead but choose apart."""
    ids = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        logits = net(input_ids=ids).logits[0, len(prompt) - 1 :].float()
    top = logits.topk(2, dim=-1)
    choice, lead = top.indices[:, 0].tolist(), (top.values[:, 0] - top.values[:, 1]).tolist()
    gap = (top.values[:, 0] - logits.gather(1, torch.tensor(made, device=logits.device)[:, None])[:, 0]).tolist()
    key = (tuple(prompt), tuple(made))
    if key not in memo:
        memo[key] = _decoded(net, prompt, made)
    d_choice, d_lead, d_gap = (memo[key][:, k].tolist() for k in range(3))
    forward = [t for t in range(len(made)) if lead[t] >= CLEAR]
    both = [t for t in forward if d_lead[t] >= CLEAR]
    clear = [t for t in both if int(d_choice[t]) == choice[t]]
    wrong = [t for t in clear if choice[t] != made[t]]
    misses = [t for t in forward if choice[t] != made[t]]
    found: dict[str, object] = {
        "clear": len(clear), "agreed": len(clear) - len(wrong), "first_wrong": wrong[0] if wrong else None,
        "forward_clear": len(forward), "forward_agreed": len(forward) - len(misses),
        "split": [t for t in both if int(d_choice[t]) != choice[t]][:8], "lead": [round(x, 3) for x in lead[:12]],
    }  # fmt: skip
    if misses:
        found["misses"] = [{"at": t, "lead": round(lead[t], 3), "gap": round(gap[t], 3), "top": round(float(top.values[t, 0]), 3),
                            "decoded_lead": round(d_lead[t], 3), "decoded_gap": round(d_gap[t], 3),
                            "decoded_takes_made": int(d_choice[t]) == made[t]} for t in misses[:8]]  # fmt: skip
    return found


def _decoded(net: torch.nn.Module, prompt: list[int], made: list[int]) -> torch.Tensor:
    """Transformers' cached decode of `prompt` then `made`, a token at a time: at each position its top choice, its
    lead over the next, and how far `made`'s token sits below its top, as `[len(made), 3]` floats."""
    rows: list[torch.Tensor] = []
    with torch.inference_mode():
        out = net(input_ids=torch.tensor([prompt], device=net.device), use_cache=True)
        for t, token in enumerate(made):
            last = out.logits[0, -1].float()
            v, i = last.topk(2)
            rows.append(torch.stack([i[0].float(), v[0] - v[1], v[0] - last[token]]))
            if t + 1 < len(made):
                out = net(input_ids=torch.tensor([[token]], device=net.device), past_key_values=out.past_key_values,
                          use_cache=True)  # fmt: skip
    return torch.stack(rows).cpu()


def _profile(decoder: object, ids: list[int]) -> dict[str, object]:
    """One warm 64-token call of `decoder` under the torch profiler (CUDA activity): its kernels and device time a
    step, and the kernels taking the most device time."""
    from torch.profiler import ProfilerActivity, profile

    from cozy_runtime.engine.decoder import Request

    with profile(activities=[ProfilerActivity.CUDA]) as prof:
        made = decoder(Request(prompts=[ids], max_new=64, stops=[]))  # type: ignore[operator]  # a Decoder
        torch.cuda.synchronize()
    kernels = [(e.key, e.count, float(getattr(e, "device_time_total", 0.0) or getattr(e, "cuda_time_total", 0.0)))
               for e in prof.key_averages()]
    kernels = [k for k in kernels if k[2] > 0]
    kernels.sort(key=lambda k: -k[2])
    steps = max(made.steps, 1)
    return {"steps": made.steps, "kernels_a_step": round(sum(k[1] for k in kernels) / steps, 1),
            "device_ms_a_step": round(sum(k[2] for k in kernels) / 1e3 / steps, 3),
            "top": [[n[:90], c, round(t / 1e3, 3)] for n, c, t in kernels[:30]]}


@app.entrypoint(accelerator=True)
def engine(ctx: Context, model: VisionLM) -> Agreement:
    """The engine's decoder (xs-213 phase 2, `cozy_runtime.engine.decoder`) on `PROMPT`: 512 greedy tokens, never
    stopping, drafted with the checkpoint's MTP head and plain. Each is timed warm (after the same call once, which records
    its step): `s` is the call's wall time, prefill included; `decode_s` and `steps` the decode's. Each mode runs on the
    path this driver's check judged (`engine.launcher`: the launcher's WHILE node, else replayed), through the model's
    declared set where it landed (`declared`), and, as `<mode>-replayed`, with the host replaying the step's graph (the same step, the same
    pod); the drafted mode also as `drafted-unfused`, replayed, with the step's small ops as torch's (the reference the
    fused kernels are held against). Every call's greedy tokens are held against Transformers' teacher-forced forward
    (`forced`), and against today's path on the same pod (`lm.generate`, drafted, warm): the first position they differ
    at, or their length. `profile` (fused) and `profile_unfused` are replayed calls; `profile_launched` the launcher's."""
    from cozy_runtime.engine.decoder import Decoder, Request
    from cozy_runtime.engine.launch import JIT

    net = model.pipe.components["model"]
    ids = model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]
    run = getattr(model.engine, "run", None)  # the model's declared set where CozyEngine installed one, as a call's
    launches = _launches(model)  # the launcher's arm on the path a call takes on this driver
    rows: dict[str, object] = {"prompt_tokens": len(ids), "gpu": torch.cuda.get_device_name(), "declared": run is not None}
    cold, _ = _timed(net, torch.tensor([ids]), 512)  # the executor's first call: today's path as a user's first waits
    seconds, today = _timed(net, torch.tensor([ids]), 512)
    rows["today"] = {"tokens": len(today), "s": round(seconds, 3), "tok_s": round(len(today) / seconds, 1),
                     "cold_s": round(cold, 3)}
    head = getattr(net, "mtp", None)
    sampled: dict[str, list[int]] = {}
    made_by: dict[str, list[int]] = {}
    decoders: dict[str, object] = {}
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del net.mtp
        try:
            request = Request(prompts=[ids], max_new=512, stops=[])
            began = time.perf_counter()
            replayed = decoders[f"{mode}-replayed"] = Decoder(net, max_rows=1, max_tokens=len(ids) + 600, launcher=False,
                                                               run=run or JIT)
            replayed(request)
            torch.cuda.synchronize()
            if mode == "drafted" or head is None:  # the engine's first call: its kernels compiled, its step captured
                rows["engine_cold_s"] = round(time.perf_counter() - began, 3)
            began = time.perf_counter()
            again = replayed(request)
            torch.cuda.synchronize()
            rows[f"{mode}-replayed"] = {"s": round(time.perf_counter() - began, 3), "decode_s": round(again.decode_s, 3),
                                        "steps": again.steps, "ms_step": round(1e3 * again.decode_s / max(again.steps, 1), 3),
                                        "same_as_launched": None, "captured": again.captured}
            replayed_tokens = again.tokens[0]
            made_by[f"{mode}-replayed"] = replayed_tokens
            if mode == "drafted":
                unfused = decoders["drafted-unfused"] = Decoder(net, max_rows=1, max_tokens=len(ids) + 600, launcher=False,
                                                                     fused=False, run=run or JIT)
                unfused(request)
                torch.cuda.synchronize()
                began = time.perf_counter()
                plain_ops = unfused(request)
                torch.cuda.synchronize()
                rows["drafted-unfused"] = {"s": round(time.perf_counter() - began, 3), "decode_s": round(plain_ops.decode_s, 3),
                                           "steps": plain_ops.steps,
                                           "ms_step": round(1e3 * plain_ops.decode_s / max(plain_ops.steps, 1), 3)}
                made_by["drafted-unfused"] = plain_ops.tokens[0]
            try:  # the launcher's arm: a failure is told and the replayed arms stand
                decoder = decoders[mode] = Decoder(net, max_rows=1, max_tokens=len(ids) + 600, run=run or JIT,
                                                    launcher=launches)
                decoder(request)
                torch.cuda.synchronize()
                began = time.perf_counter()
                made = decoder(request)
                torch.cuda.synchronize()
                seconds = time.perf_counter() - began
            except Exception as exc:
                rows[mode] = {"error": f"{type(exc).__name__}: {exc}"[:600]}
                decoders.pop(mode, None)
                continue
        finally:
            if head is not None:
                net.mtp = head
        tokens = made.tokens[0]
        made_by[mode] = tokens
        kept = made.kept[0] if made.kept else []
        drawn = decoder(Request(prompts=[ids], max_new=512, stops=[], temperature=1.0, top_k=20, top_p=0.95, seed=213))
        drawn_kept = drawn.kept[0] if drawn.kept else []
        sampled[mode] = drawn.tokens[0]
        rows[f"{mode}-sampled"] = {"tokens": len(drawn.tokens[0]), "steps": drawn.steps,
                                   "kept_a_step": round(sum(drawn_kept) / max(len(drawn_kept), 1), 3)}
        rows[mode] = {
            "tokens": len(tokens), "s": round(seconds, 3), "tok_s": round(len(tokens) / seconds, 1),
            "prefill_s": round(made.prefill_s, 4), "decode_s": round(made.decode_s, 3), "steps": made.steps,
            "ms_step": round(1e3 * made.decode_s / max(made.steps, 1), 3), "captured": made.captured, "notes": made.notes,
            "kept_a_step": round(sum(kept) / max(len(kept), 1), 3),
            "same_as_today": next((i for i, (x, y) in enumerate(zip(tokens, today)) if x != y), min(len(tokens), len(today))),
            "launcher": getattr(decoder, "launcher", None),
        }
        replayed_row = rows[f"{mode}-replayed"]
        assert isinstance(replayed_row, dict)
        replayed_row["same_as_launched"] = next((i for i, (x, y) in enumerate(zip(tokens, replayed_tokens)) if x != y),
                                                min(len(tokens), len(replayed_tokens)))
    rows["profile"] = _profile(decoders.get("drafted-replayed") or decoders["plain-replayed"], ids)
    if "drafted" in decoders or "plain" in decoders:
        rows["profile_launched"] = _profile(decoders.get("drafted") or decoders["plain"], ids)
    if "drafted-unfused" in decoders:
        rows["profile_unfused"] = _profile(decoders["drafted-unfused"], ids)
    memo: dict[tuple[tuple[int, ...], tuple[int, ...]], torch.Tensor] = {}
    rows["forced"] = {"today": _forced(net, ids, today, memo)}
    for mode in made_by:
        rows["forced"][mode] = _forced(net, ids, made_by[mode], memo)
    if len(sampled) == 2:  # the same noise at each index: told, not required (a near tie in the scores can turn a draw)
        a, b = sampled["drafted"], sampled["plain"]
        rows["sampled_same"] = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
    return Agreement(json.dumps(rows))


#: Four asks of different lengths, as Qwen's chat template writes a user turn with thinking off: one bucket's rows.
ASKS = (
    PROMPT,
    "<|im_start|>user\nName three colours.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
    f"<|im_start|>user\n{TEXTS[0]}\n\nRetell this as a short poem.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
    f"<|im_start|>user\n{TEXTS[2]}\nExplain what this function does, step by step.<|im_end|>\n<|im_start|>assistant\n"
    "<think>\n\n</think>\n\n",
)


@app.entrypoint(accelerator=True)
def bucket(ctx: Context, model: VisionLM) -> Agreement:
    """`_bucket` on the path the engine judged for this driver (`engine.launcher`), as a call decodes: the launcher,
    else the host-replayed graph; it waits for the check."""
    return _bucket(model, launcher=_launches(model))


def _launches(model: VisionLM) -> bool:
    """Whether the engine launches its step on this driver (`engine.launcher`'s judgement, waited for), or replays it
    from the host; raises where it runs neither."""
    from cozy_runtime.engine import launcher

    index = int(getattr(model.engine, "index", 0))
    deadline = time.monotonic() + 300
    while (paths := launcher.judged(index)) is None:
        if time.monotonic() > deadline:
            raise RuntimeError("the engine's launcher check never ended")
        time.sleep(1)
    if paths.launcher and paths.replayed:
        raise RuntimeError(f"this driver runs neither launch path: {paths.launcher}; {paths.replayed}")
    return not paths.launcher


@app.entrypoint(accelerator=True)
def bucket_replayed(ctx: Context, model: VisionLM) -> Agreement:
    """`_bucket` with the host replaying the step's graph (`Decoder(launcher=False)`): no WHILE node."""
    return _bucket(model, launcher=False)


def _bucket(model: VisionLM, launcher: bool) -> Agreement:
    """The engine's decoder on `ASKS` as four rows of one bucket (a call `vlm.ask` serves: up to 16 rows), 256 greedy
    tokens each, drafted and plain: each row held against Transformers' teacher-forced forward at its clear positions
    (`forced`), and the same row decoded alone (a bucket of one) held the same way, with where the two first differ.
    Its kernels are the model's declared set where CozyEngine installed one (`declared`), as a call's are."""
    from cozy_runtime.engine.decoder import Decoder, Request
    from cozy_runtime.engine.launch import JIT

    net = model.pipe.components["model"]
    prompts = [model.processor.tokenizer(a, add_special_tokens=False)["input_ids"] for a in ASKS]
    run = getattr(model.engine, "run", None)
    rows: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "prompt_tokens": [len(p) for p in prompts],
                               "declared": run is not None, "launcher": launcher}
    memo: dict[tuple[tuple[int, ...], tuple[int, ...]], torch.Tensor] = {}
    head = getattr(net, "mtp", None)
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del net.mtp
        try:
            decoder = Decoder(net, max_rows=4, max_tokens=max(len(p) for p in prompts) + 300, run=run or JIT,
                              launcher=launcher)
            made = decoder(Request(prompts=prompts, max_new=256, stops=[])).tokens
            alone = [decoder(Request(prompts=[p], max_new=256, stops=[])).tokens[0] for p in prompts]
        finally:
            if head is not None:
                net.mtp = head
        rows[mode] = [
            {"bucket": _forced(net, p, made[i], memo), "alone": _forced(net, p, alone[i], memo),
             "same_as_alone": next((t for t, (x, y) in enumerate(zip(made[i], alone[i])) if x != y), None)}
            for i, p in enumerate(prompts)
        ]  # fmt: skip
        # Every token made, as one digest: launched and replayed runs of the same rows compare by it.
        rows[f"{mode}_digest"] = hashlib.sha256(json.dumps([made, alone]).encode()).hexdigest()[:16]
        rows[f"{mode}_launched"] = bool(decoder.launcher)  # whether the decoder still launches after these calls
    return Agreement(json.dumps(rows))


@app.entrypoint(accelerator=True)
def rows(ctx: Context, model: VisionLM) -> Agreement:
    """`VisionLM.ask` with 17 questions in one call, one more than the engine decodes at once (xs-213's fallback routes,
    the engine-routes session): every row's answer, and which engine `VisionLM` holds; the run's `.engine` note says which
    served the call and why."""
    from cozy_runtime.vlm import Question

    questions = [Question(text=f"In one sentence: what is {n} plus {n}?") for n in range(17)]
    answers = model.ask(questions, system="", thinking=False, stop=[], max_new_tokens=24, presence_penalty=0.0,
                        temperature=0.0, top_p=1.0, seed=0)  # fmt: skip
    return Agreement(json.dumps({"engine": type(model.engine).__name__, "rows": len(answers),
                                 "answered": sum(bool(a.strip()) for a in answers), "first": answers[0][:160]}))


class _Digits:
    """An `lm.Index` (outlines-core's shape) over a tokenizer's vocabulary: one to eight digit tokens, then `stop`.
    State n has made n digits; state 9 has stopped."""

    def __init__(self, vocab: dict[str, int], stop: int) -> None:
        self.digits = sorted(i for t, i in vocab.items() if t.isascii() and t.isdigit())
        self.stop = stop

    def get_initial_state(self) -> int:
        return 0

    def get_next_state(self, state: int, token: int) -> int | None:
        if token in self.digits and state < 8:
            return state + 1
        return 9 if token == self.stop and 1 <= state <= 8 else None

    def get_allowed_tokens(self, state: int) -> list[int] | None:
        return self.digits if state == 0 else [*self.digits, self.stop] if state < 8 else [self.stop]


class _NoDigits:
    """An `lm.Index` of `[^0-9]+` then `stop`, over a tokenizer's vocabulary: a JSON string's loop, nearly every token
    allowed at each step. State 0 has made nothing, 1 is inside, 2 has stopped."""

    def __init__(self, vocab: dict[str, int], decode: Callable[[int], str], stop: int, specials: set[int]) -> None:
        self.words = sorted(i for i in vocab.values() if i not in specials and not any(c.isdigit() for c in decode(i)))
        self.member, self.stop = frozenset(self.words), stop

    def get_initial_state(self) -> int:
        return 0

    def get_next_state(self, state: int, token: int) -> int | None:
        if state < 2 and token in self.member:
            return 1
        return 2 if state == 1 and token == self.stop else None

    def get_allowed_tokens(self, state: int) -> list[int] | None:
        return self.words if state == 0 else [*self.words, self.stop] if state == 1 else [self.stop]


class _Replaying:
    """cr-351's `lm.Grammar` as it was before xs-213's follow-up: one prefix's state kept, the allowed ids a new list
    each ask. The A arm of `grammar-cost`, so both arms run on one pod and one Runtime."""

    def __init__(self, index: _NoDigits) -> None:
        self.index = index
        self.last: tuple[tuple[int, ...], int] = ((), index.get_initial_state())

    def allowed(self, tokens: list[int]) -> list[int]:
        held = tuple(tokens)
        done, state = self.last
        if held[: len(done)] != done:
            done, state = (), self.index.get_initial_state()
        for token in held[len(done) :]:
            following = self.index.get_next_state(state, token)
            if following is None:
                raise ValueError(f"token {token} is outside the grammar")
            state = following
        self.last = (held, state)
        ids = self.index.get_allowed_tokens(state)
        if ids is None:
            raise ValueError(f"state {state} is outside the grammar")
        return list(ids)


class _Any:
    """A constraint that allows any token: the baseline arm, on the same path a held call takes (Transformers'
    engine, its host caught up with each step) with no mask to build."""

    def allowed(self, tokens: list[int]) -> None:
        return None


def _grammar_cost(model: VisionLM) -> Agreement:
    """Four rows sharing one constraint, 128 greedy tokens each, warm: held by `_Any`, by `lm.Grammar`, and by
    `_Replaying` (the old Grammar): each arm's wall seconds and its cost a token a row over `_Any`'s. Both grammars
    must give the same answers, with no digit in them."""
    from cozy_runtime.vlm import Question

    tokenizer = model.processor.tokenizer
    stop = int(tokenizer.convert_tokens_to_ids("<|im_end|>"))
    index = _NoDigits(tokenizer.get_vocab(), lambda i: tokenizer.decode([i]), stop, set(tokenizer.all_special_ids))
    questions = [Question(text=t) for t in ("Describe a lighthouse at night.", "Describe a busy market.",
                                            "Describe a mountain lake.", "Describe a train station.")]

    def timed(allowed: object) -> tuple[float, list[str]]:
        def run() -> list[str]:
            return model.ask(questions, system="", thinking=False, stop=[], max_new_tokens=128, presence_penalty=0.0,
                             temperature=0.0, top_p=1.0, seed=0, allowed=allowed)  # type: ignore[arg-type]  # a Constraint

        run()
        torch.cuda.synchronize()
        began = time.perf_counter()
        answers = run()
        torch.cuda.synchronize()
        return time.perf_counter() - began, answers

    plain_s, _ = timed(_Any())
    new_s, new = timed(lm.Grammar(index))
    old_s, old = timed(_Replaying(index))
    if new != old:
        raise RuntimeError(f"grammar-cost: lm.Grammar and the old Grammar answered differently: {new[0][:80]!r} / {old[0][:80]!r}")
    if any(c.isdigit() for a in new for c in a):
        raise RuntimeError("grammar-cost: a digit in a grammar-held answer")
    per = 1e3 / (128 * len(questions))
    return Agreement(json.dumps({
        "engine": type(model.engine).__name__, "case": "grammar-cost", "allowed_ids": len(index.words),
        "plain_s": round(plain_s, 3), "grammar_s": round(new_s, 3), "old_grammar_s": round(old_s, 3),
        "grammar_ms_token_row": round((new_s - plain_s) * per, 3), "old_grammar_ms_token_row": round((old_s - plain_s) * per, 3),
        "first": new[0][:120],
    }))


class Route(msgspec.Struct):
    #: `grammar`: a call held to `_Digits` (`allowed`); `conversation`: a text question after a turn with a picture;
    #: `ngram`: a call with `no_repeat_ngram`; `ceiling`: 512 tokens asked of an engine whose rows hold 256 (its
    #: clamp warning is in the run's record);
    #: `grammar-cost`: a grammar's cost a token (`_grammar_cost`).
    case: str = "grammar"


@app.entrypoint(accelerator=True)
def routes(ctx: Context, payload: Route, model: VisionLM) -> Agreement:
    """One `VisionLM.ask` that CozyEngine hands to Transformers' engine (the engine-routes session): its answer, and
    which engine `VisionLM` holds; the run's `.engine` note says which served it and why. The grammar's answer must be
    digits only, though "Name a colour." asks for a word: proof the constraint held."""
    from PIL import Image

    from cozy_runtime.vlm import Question, Turn

    if payload.case == "grammar-cost":
        return _grammar_cost(model)
    tokenizer = model.processor.tokenizer
    allowed: lm.Grammar | None = None
    ngram: int | None = None
    question = Question(text="Name a colour.")
    if payload.case == "grammar":
        allowed = lm.Grammar(_Digits(tokenizer.get_vocab(), tokenizer.convert_tokens_to_ids("<|im_end|>")))
    elif payload.case == "conversation":
        square = Image.new("RGB", (256, 256), (200, 30, 30))
        question = Question(text="What colour was the square?", turns=[
            Turn("user", ["Hello."]), Turn("assistant", ["Hello! How can I help?"]),
            Turn("user", [square, "Remember this picture."]), Turn("assistant", ["I will."]),
        ])  # fmt: skip
    elif payload.case == "ngram":
        ngram = 3
    elif payload.case == "ceiling":  # the engine serves it, its length clamped at the start, with a warning
        from cozy_runtime.engine.decoder import Decoder

        model.engine.decoder = Decoder(model.pipe.components["model"], max_rows=16, max_tokens=256,  # type: ignore[attr-defined]  # CozyEngine's
                                       run=model.engine.run)  # type: ignore[attr-defined]  # its kernel set
        question = Question(text=PROMPT.split("\n")[1].removesuffix("<|im_end|>"))
    else:
        raise ValueError(f"no route '{payload.case}'")
    try:
        answer = model.ask([question], system="", thinking=False, stop=[], presence_penalty=0.0, temperature=0.0,
                           max_new_tokens=512 if payload.case == "ceiling" else 24, top_p=1.0, seed=0,
                           allowed=allowed, no_repeat_ngram=ngram)[0]  # fmt: skip
    finally:
        if payload.case == "ceiling":
            model.engine.decoder = None  # type: ignore[attr-defined]  # the next call's is the model's own
    if not answer.strip():
        raise RuntimeError(f"{payload.case}: no answer")
    if payload.case == "grammar" and not answer.strip().isdigit():
        raise RuntimeError(f"grammar: the answer {answer!r} is not digits: the constraint didn't hold")
    return Agreement(json.dumps({"engine": type(model.engine).__name__, "case": payload.case, "answer": answer[:160]}))


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


#: int8_paths' prompt: about a thousand tokens of text in one user turn (TEXTS over again), thinking off.
LONG_TOKENS = 1000
#: The tokens int8_paths decodes and holds every path to.
PATH_TOKENS = 48


def _long_prompt(tokenizer: object) -> list[int]:
    body, ids = "", []
    while len(ids) < LONG_TOKENS:
        body += "\n\n".join(TEXTS) + "\n\n"
        text = (f"<|im_start|>user\n{body}Retell all of the above in your own words.<|im_end|>\n<|im_start|>assistant\n"
                "<think>\n\n</think>\n\n")  # fmt: skip
        ids = tokenizer(text, add_special_tokens=False)["input_ids"]  # type: ignore[operator]  # a tokenizer
    return list(ids)


def _hadamard256(values: torch.Tensor) -> torch.Tensor:
    """The regular Hadamard H4⊗H4⊗H4⊗H4 on each 256 channels, one 4-point stage of additions a factor, as the Runtime's
    `int8.convrot` eager law writes it (`encoding.formats.hadamard`)."""
    shape = values.shape
    work = values.reshape(-1, 4, 4, 4, 4)
    for axis in range(1, 5):
        a, b, c, d = work.unbind(axis)
        work = torch.stack((a + b + c - d, a + b - c + d, a - b + c + d, b + c + d - a), axis)
    return work.reshape(shape)


def _eager(leaf: torch.nn.Module, x: torch.Tensor) -> torch.Tensor:
    """`int8.convrot`'s W8A8 law in torch ops (the Runtime's `int8_linear.eager`): each row rotated (H/16 per 256),
    quantized per row (amax / 127, round half even), torch's int8 GEMM, both scales: no row depends on another."""
    data, scale = leaf.data, leaf.scale  # type: ignore[union-attr]  # an int8 leaf's buffers
    width = x.shape[-1]
    rotated = _hadamard256(x.reshape(-1, width).float()).mul_(1.0 / 16)
    row_scale = (rotated.abs().amax(-1, keepdim=True) / 127.0).clamp_(min=1e-12)
    quantized = torch.round(rotated / row_scale).clamp_(-127, 127).to(torch.int8)
    rows = int(quantized.shape[0])
    padded = torch.nn.functional.pad(quantized, (0, 0, 0, max(0, 17 - rows)))
    acc = torch._int_mm(padded, data.t())[:rows]
    out = acc.float().mul_(row_scale.reshape(-1, 1)).mul_(scale.float().reshape(1, -1))
    return out.to(torch.bfloat16).reshape(*x.shape[:-1], -1)


def _int8_leaves(net: torch.nn.Module) -> dict[str, torch.nn.Module]:
    return {name: m for name, m in net.named_modules() if getattr(m, "encoding", None) == "int8.convrot"}


def _bits(a: torch.Tensor, b: torch.Tensor) -> dict[str, object]:
    """Two outputs of the same rows: rows bit for bit equal, elements that differ, the largest difference."""
    a, b = a.reshape(-1, a.shape[-1]), b.reshape(-1, b.shape[-1])
    same = (a == b) | (a.isnan() & b.isnan())
    return {"rows": int(a.shape[0]), "rows_equal": int(same.all(-1).sum()), "elements_differ": int((~same).sum()),
            "max_abs": round(float((a.float() - b.float()).abs().max()), 5),
            "max_rel": round(float((a.float() - b.float()).abs().max() / a.float().abs().max().clamp(min=1e-30)), 7)}


def _kernel_paths(net: torch.nn.Module, prompt: list[int], leaves: dict[str, torch.nn.Module]) -> dict[str, object]:
    """On rows the 9B's own forward hands a few int8 leaves (the long prompt), each path the leaf takes by row count
    against the others, bit for bit: the prefill's quantize and tiled GEMM (all rows at once, `gemm`), a decode's GEMV
    (one row, `gemv1`; four, `gemv4`), the GEMM on five rows (`gemm5`), and the eager law (`eager`). Then the fused
    norm prologue (`accel.kernels.int8_rows`, the RMSNorm in f32 inside the quantizer) against the leaf on the model's
    own bf16 RMSNorm output, on the rows a post-attention norm hands gate_proj."""
    from cozy_runtime.accel import kernels

    root = net.get_decoder()  # type: ignore[operator]  # a Transformers model's
    count = len(root.layers)
    picked = [f"layers.{i}." for i in sorted({0, 3, count // 2, count - 1})]
    chosen = {n: m for n, m in leaves.items() if any(p in n for p in picked) and n.rsplit(".", 1)[-1] in
              ("gate_proj", "down_proj", "q_proj", "o_proj", "in_proj_qkv", "out_proj")}  # fmt: skip
    seen: dict[str, torch.Tensor] = {}
    norms: dict[int, torch.Tensor] = {}
    hooks = [m.register_forward_pre_hook(lambda mod, args, name=n: seen.setdefault(name, args[0].detach().clone()))
             for n, m in chosen.items()]  # fmt: skip
    for i in (0, count // 2):
        hooks.append(root.layers[i].post_attention_layernorm.register_forward_pre_hook(
            lambda mod, args, i=i: norms.setdefault(i, args[0].detach().clone())))  # fmt: skip
    try:
        with torch.inference_mode():
            net(input_ids=torch.tensor([prompt], device=net.device), logits_to_keep=1)
    finally:
        for hook in hooks:
            hook.remove()
    rows: dict[str, object] = {}
    sample = [*range(32), *range(len(prompt) - 32, len(prompt))]
    with torch.inference_mode():
        for name, x in seen.items():
            leaf = chosen[name]
            flat = x.reshape(-1, x.shape[-1])
            try:
                whole = leaf(flat)[sample]
                gemv1 = torch.cat([leaf(flat[i : i + 1]) for i in sample])
                gemv4 = torch.cat([leaf(flat[sample[j : j + 4]]) for j in range(0, len(sample), 4)])
                gemm5 = torch.cat([leaf(flat[[*sample[j : j + 4], sample[j]]])[:4] for j in range(0, len(sample), 4)])
                eager = _eager(leaf, flat[sample])
                rows[name] = {"k": int(flat.shape[-1]), "gemv1": _bits(whole, gemv1), "gemv4": _bits(whole, gemv4),
                              "gemm5": _bits(whole, gemm5), "eager": _bits(whole, eager)}  # fmt: skip
            except Exception as exc:  # told; the other leaves stand
                rows[name] = f"{type(exc).__name__}: {exc}"[:300]
        for i, x in norms.items():
            layer = root.layers[i]
            norm, gate = layer.post_attention_layernorm, layer.mlp.gate_proj
            flat = x.reshape(-1, x.shape[-1])
            try:
                unfused = gate(norm(flat))
                operand = kernels.int8_rows(flat, (gate,), norm="rms", eps=float(norm.eps),
                                            scale=norm.weight.float().to(flat.dtype).reshape(1, -1))  # fmt: skip  # a copy: the op reads it by pointer
                if operand is None:
                    rows[f"prologue.{i}"] = "declined: int8_rows gave no operand"
                    continue
                fused = gate(operand)
                rows[f"prologue.{i}"] = {"fused_vs_unfused": _bits(unfused, fused),
                                         "fused_vs_eager_on_f32_norm": _bits(fused, _eager(gate, _rms_f32(norm, flat)))}
            except Exception as exc:
                rows[f"prologue.{i}"] = f"{type(exc).__name__}: {exc}"[:300]
    return rows


def _rms_f32(norm: torch.nn.Module, x: torch.Tensor) -> torch.Tensor:
    """Qwen3.5's RMSNorm left in f32 (its forward rounds to bf16 at the end)."""
    f = x.float()
    return f * torch.rsqrt(f.pow(2).mean(-1, keepdim=True) + float(norm.eps)) * (1.0 + norm.weight.float())


def _forward(net: torch.nn.Module, prompt: list[int], made: list[int]) -> tuple[torch.Tensor, torch.Tensor]:
    """Transformers' one-pass forward over the prompt and `made`: the logits each of `made` was chosen from (f32,
    `[len(made), V]`) and each layer's hidden state at those positions (`[layers + 1, len(made), H]`), on the host."""
    ids = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        out = net(input_ids=ids, output_hidden_states=True, logits_to_keep=len(made))
    hidden = torch.stack([h[0, len(prompt) - 1 :].float() for h in out.hidden_states])
    return out.logits[0].float().cpu(), hidden.cpu()


def _decode(net: torch.nn.Module, prompt: list[int], made: list[int] | None) -> tuple[torch.Tensor, torch.Tensor, list[int]]:
    """Transformers' cached decode, a token at a time: of `made` (teacher-forced), or greedy where `made` is None, for
    `PATH_TOKENS`. The same logits and hidden states as `_forward`, and the tokens."""
    logits, hidden, tokens = [], [], []
    count = PATH_TOKENS if made is None else len(made)
    with torch.inference_mode():
        out = net(input_ids=torch.tensor([prompt], device=net.device), use_cache=True, output_hidden_states=True,
                  logits_to_keep=1)  # fmt: skip
        for t in range(count):
            last = out.logits[0, -1].float()
            logits.append(last.cpu())
            hidden.append(torch.stack([h[0, -1].float() for h in out.hidden_states]).cpu())
            token = int(last.argmax()) if made is None else made[t]
            tokens.append(token)
            if t + 1 < count:
                out = net(input_ids=torch.tensor([[token]], device=net.device), past_key_values=out.past_key_values,
                          use_cache=True, output_hidden_states=True, logits_to_keep=1)  # fmt: skip
    return torch.stack(logits), torch.stack(hidden, 1), tokens


def _gap(a: torch.Tensor, b: torch.Tensor, ha: torch.Tensor | None = None, hb: torch.Tensor | None = None,
         skip: int = 0) -> dict[str, object]:  # fmt: skip
    """Logits `a` against `b` at each position past `skip`: the largest difference (max and mean over positions), the
    tops that agree, KL(a || b) in nats, and the lead `a` gives its top over `b`'s top where they differ. With hidden
    states, each layer's relative difference (the largest over positions)."""
    a, b = a[skip:], b[skip:]
    diff = (a - b).abs().amax(-1)
    ta, tb = a.argmax(-1), b.argmax(-1)
    kl = (torch.log_softmax(a, -1).exp() * (torch.log_softmax(a, -1) - torch.log_softmax(b, -1))).sum(-1)
    found: dict[str, object] = {"positions": int(a.shape[0]), "max": round(float(diff.max()), 3),
                                "mean": round(float(diff.mean()), 3), "tops_agree": int((ta == tb).sum()),
                                "kl_mean": round(float(kl.mean()), 5), "kl_max": round(float(kl.max()), 5)}  # fmt: skip
    if ha is not None and hb is not None:
        ha, hb = ha[:, skip:], hb[:, skip:]
        rel = ((ha - hb).norm(dim=-1) / ha.norm(dim=-1).clamp(min=1e-30)).amax(-1)
        found["hidden_rel"] = [round(float(r), 5) for r in rel]
    return found


@app.entrypoint(accelerator=True)
def int8_paths(ctx: Context, reference: VisionLM, model: VisionLM) -> Agreement:
    """Where int8.convrot's one-pass forward and its token-by-token decode part on a long prompt (xs-213): `model` an
    int8 lane, `reference` the same checkpoint's bf16 lane, a ~1,000-token text prompt, `PATH_TOKENS` tokens.
    - `kernels`: each int8 path the leaf takes by row count, on the same real rows, bit for bit (`_kernel_paths`).
    - `bf16`: the reference's own forward against its own decode (the paths' difference with no 8-bit operand).
    - `int8`: the lane's forward against its decode, as Transformers runs both (prefill's GEMM, decode's GEMV);
      `int8_decode_gemm`: its decode with every int8 leaf given five rows, the prefill's quantize and GEMM, so both
      paths run one kernel; `int8_eager`: forward and decode both on the eager law.
    - `to_bf16`: each int8 path's logits against the reference's forward, and the reference's decode against it.
    Every arm reads the same tokens: the reference's greedy decode. Hidden states per layer say where the paths part."""
    ref_net, net = reference.pipe.components["model"], model.pipe.components["model"]
    prompt = _long_prompt(model.processor.tokenizer)
    found: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "prompt_tokens": len(prompt), "tokens": PATH_TOKENS}
    ref_dec, ref_dec_h, made = _decode(ref_net, prompt, None)
    ref_fwd, ref_fwd_h = _forward(ref_net, prompt, made)
    found["bf16"] = _gap(ref_fwd, ref_dec, ref_fwd_h, ref_dec_h, skip=1)
    leaves = _int8_leaves(net)
    found["int8_leaves"] = len(leaves)
    try:
        found["kernels"] = _kernel_paths(net, prompt, leaves)
    except Exception as exc:
        found["kernels"] = f"{type(exc).__name__}: {exc}"[:300]
    fwd, fwd_h = _forward(net, prompt, made)
    dec, dec_h, _ = _decode(net, prompt, made)
    found["int8"] = _gap(fwd, dec, fwd_h, dec_h, skip=1)
    arms = {"int8_forward": fwd, "int8_decode": dec}
    shapes: dict[int, torch.Size] = {}

    def pad(mod: torch.nn.Module, args: tuple[object, ...]) -> tuple[object, ...] | None:
        x = args[0]
        if not isinstance(x, torch.Tensor) or x.numel() > 4 * x.shape[-1]:
            return None
        flat = x.reshape(-1, x.shape[-1])
        shapes[id(mod)] = x.shape
        return (torch.cat([flat, flat[-1:].expand(5 - flat.shape[0], -1)]), *args[1:])

    def cut(mod: torch.nn.Module, args: tuple[object, ...], out: torch.Tensor) -> torch.Tensor | None:
        shape = shapes.pop(id(mod), None)
        return None if shape is None else out[: shape.numel() // shape[-1]].reshape(*shape[:-1], out.shape[-1])

    hooks = [h for m in leaves.values() for h in (m.register_forward_pre_hook(pad), m.register_forward_hook(cut))]
    try:
        dec5, dec5_h, _ = _decode(net, prompt, made)
        found["int8_decode_gemm"] = _gap(fwd, dec5, fwd_h, dec5_h, skip=1)
        found["decode_gemm_vs_gemv"] = _gap(dec, dec5, dec_h, dec5_h)
        arms["int8_decode_gemm"] = dec5
    except Exception as exc:
        found["int8_decode_gemm"] = f"{type(exc).__name__}: {exc}"[:300]
    finally:
        for hook in hooks:
            hook.remove()
    for leaf in leaves.values():
        leaf.forward = lambda x, *args, leaf=leaf, **kwargs: _eager(leaf, x)  # type: ignore[method-assign]
    try:
        efwd, efwd_h = _forward(net, prompt, made)
        edec, edec_h, _ = _decode(net, prompt, made)
        found["int8_eager"] = _gap(efwd, edec, efwd_h, edec_h, skip=1)
        found["eager_vs_kernels_forward"] = _gap(fwd, efwd, fwd_h, efwd_h)
        arms["int8_eager_forward"], arms["int8_eager_decode"] = efwd, edec
    except Exception as exc:
        found["int8_eager"] = f"{type(exc).__name__}: {exc}"[:300]
    finally:
        for leaf in leaves.values():
            leaf.__dict__.pop("forward", None)
    found["to_bf16"] = {name: _gap(ref_fwd, logits) for name, logits in {"bf16_decode": ref_dec, **arms}.items()}
    return Agreement(json.dumps(found))


def _kv(cache: object) -> dict[int, tuple[torch.Tensor, torch.Tensor]]:
    """A Transformers cache's attention layers' keys and values, by layer."""
    found = {}
    for i, layer in enumerate(getattr(cache, "layers", ())):
        keys, values = getattr(layer, "keys", None), getattr(layer, "values", None)
        if isinstance(keys, torch.Tensor) and isinstance(values, torch.Tensor) and keys.numel():
            found[i] = (keys, values)
    return found


def _kv_rel(a: dict[int, tuple[torch.Tensor, torch.Tensor]], b: dict[int, tuple[torch.Tensor, torch.Tensor]],
            prompt: int) -> dict[str, object]:  # fmt: skip
    """Each attention layer's keys and values in cache `b` against `a`'s, over the prompt's positions and the decoded
    ones apart: the largest relative difference of a position's row."""
    rows: dict[str, object] = {}
    for i in sorted(set(a) & set(b)):
        parts = {}
        for name, x, y in (("k", a[i][0], b[i][0]), ("v", a[i][1], b[i][1])):
            n = min(x.shape[-2], y.shape[-2])
            x, y = x[..., :n, :].float(), y[..., :n, :].float()
            rel = ((x - y).norm(dim=-1) / x.norm(dim=-1).clamp(min=1e-30)).amax(dim=(0, 1))
            parts[name] = [round(float(rel[:prompt].max()), 5), round(float(rel[prompt:].max()) if n > prompt else 0.0, 5)]
        rows[str(i)] = parts
    return rows


def _paging(net: torch.nn.Module, prompt: list[int]) -> dict[str, object]:
    """On one lane as it is placed now: today's path (`lm.generate`, greedy, `PATH_TOKENS`) and its tokens held to the
    one-pass forward (`today`); then Transformers' token-by-token decode of the same tokens against that forward, as
    the drives run it (`decode`: its K/V a plain cache, outside the memory manager) and with every attention layer's
    K/V held in `author.retained` between steps (`retained`). Each says the logits' gap, each layer's hidden state's
    relative difference, and each attention layer's cached keys and values against the forward's (prompt, decoded)."""
    from cozy_runtime.author import retained

    ids = torch.tensor([prompt])
    _, today = _timed(net, ids, PATH_TOKENS)
    made = today[:PATH_TOKENS]
    inputs = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        out = net(input_ids=inputs, use_cache=True, output_hidden_states=True, logits_to_keep=len(made))
    fwd = out.logits[0].float()
    fwd_h = torch.stack([h[0, len(prompt) - 1 :].float() for h in out.hidden_states])
    fwd_kv = _kv(out.past_key_values)
    del out
    top = fwd.topk(2, dim=-1)
    lead = (top.values[:, 0] - top.values[:, 1]).tolist()
    clear = [t for t in range(len(made)) if lead[t] >= CLEAR]
    missed = [t for t in clear if int(top.indices[t, 0]) != made[t]]
    found: dict[str, object] = {"today": {"tokens": len(made), "clear": len(clear), "agreed": len(clear) - len(missed),
                                          "misses": [{"at": t, "lead": round(lead[t], 3)} for t in missed[:8]],
                                          "digest": hashlib.sha256(json.dumps(made).encode()).hexdigest()[:16]}}  # fmt: skip
    for arm in ("decode", "retained"):
        logits, hidden, stores = [], [], []
        try:
            with torch.inference_mode():
                step = net(input_ids=torch.tensor([prompt], device=net.device), use_cache=True,
                           output_hidden_states=True, logits_to_keep=1)  # fmt: skip
                for t in range(len(made)):
                    logits.append(step.logits[0, -1].float())
                    hidden.append(torch.stack([h[0, -1].float() for h in step.hidden_states]))
                    cache = step.past_key_values
                    if arm == "retained":  # the attention layers' K/V the Runtime's memory holds, not plain tensors
                        held = _kv(cache)
                        store = retained(sum(k.nbytes + v.nbytes for k, v in held.values()))
                        for i, (k, v) in held.items():
                            cache.layers[i].keys, cache.layers[i].values = store.put(k, v).get()
                        del held
                        stores.append(store)
                        if len(stores) > 1:
                            stores.pop(0).close()
                    if t + 1 < len(made):
                        step = net(input_ids=torch.tensor([[made[t]]], device=net.device), past_key_values=cache,
                                   use_cache=True, output_hidden_states=True, logits_to_keep=1)  # fmt: skip
                dec, dec_h = torch.stack(logits), torch.stack(hidden, 1)
                row = _gap(fwd.cpu(), dec.cpu(), fwd_h.cpu(), dec_h.cpu(), skip=1)
                row["kv"] = _kv_rel(fwd_kv, _kv(cache), len(prompt))
                found[arm] = row
        except Exception as exc:  # told; the other arm stands
            found[arm] = f"{type(exc).__name__}: {exc}"[:300]
        finally:
            for store in stores:
                store.close()
    return found


@app.entrypoint(accelerator=True)
def paging_alone(ctx: Context, model: VisionLM) -> Agreement:
    """`_paging` on `model` alone on the card (the session's first call: nothing else placed), on int8_paths' prompt."""
    net = model.pipe.components["model"]
    prompt = _long_prompt(model.processor.tokenizer)
    return Agreement(json.dumps({"gpu": torch.cuda.get_device_name(), "prompt_tokens": len(prompt), **_paging(net, prompt)}))


@app.entrypoint(accelerator=True)
def paging_beside(ctx: Context, reference: VisionLM, model: VisionLM) -> Agreement:
    """`_paging` on `reference` (bf16) after `model` (int8) ran a forward of the prompt, so both are placed and the
    reference's weights don't all fit beside it: the paged case of int8_paths (44650186994)."""
    net, other = reference.pipe.components["model"], model.pipe.components["model"]
    prompt = _long_prompt(model.processor.tokenizer)
    with torch.inference_mode():
        other(input_ids=torch.tensor([prompt], device=other.device), logits_to_keep=1)
    return Agreement(json.dumps({"gpu": torch.cuda.get_device_name(), "prompt_tokens": len(prompt), **_paging(net, prompt)}))
