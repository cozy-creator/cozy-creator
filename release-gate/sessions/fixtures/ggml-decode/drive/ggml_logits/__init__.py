"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up)."""

import json
import time

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


def _forced(net: torch.nn.Module, prompt: list[int], made: list[int]) -> dict[str, object]:
    """`made` held against Transformers' own forward over the prompt and `made` in one pass (teacher-forced): at
    each position whose top logit leads the next by at least 0.5, whether `made` took it; the lead where it didn't."""
    ids = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        logits = net(input_ids=ids).logits[0, len(prompt) - 1 :].float()
    top = logits.topk(2, dim=-1)
    choice, lead = top.indices[:, 0].tolist(), (top.values[:, 0] - top.values[:, 1]).tolist()
    clear = [t for t in range(len(made)) if lead[t] >= 0.5]
    wrong = [t for t in clear if choice[t] != made[t]]
    return {"clear": len(clear), "agreed": len(clear) - len(wrong), "first_wrong": wrong[0] if wrong else None,
            "lead": [round(x, 3) for x in lead[:12]]}


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
    launcher's WHILE node and, as `<mode>-replayed`, with the host replaying the step's graph (the same step, the same
    pod); the drafted mode also as `drafted-unfused`, replayed, with the step's small ops as torch's (the reference the
    fused kernels are held against). Every call's greedy tokens are held against Transformers' teacher-forced forward
    (`forced`), and against today's path on the same pod (`lm.generate`, drafted, warm): the first position they differ
    at, or their length. `profile` (fused) and `profile_unfused` are replayed calls; `profile_launched` the launcher's."""
    from cozy_runtime.engine.decoder import Decoder, Request

    net = model.pipe.components["model"]
    ids = model.processor.tokenizer(PROMPT, add_special_tokens=False)["input_ids"]
    rows: dict[str, object] = {"prompt_tokens": len(ids), "gpu": torch.cuda.get_device_name()}
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
            replayed = decoders[f"{mode}-replayed"] = Decoder(net, max_rows=1, max_tokens=len(ids) + 600, launcher=False)
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
                                                                     fused=False)
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
                decoder = decoders[mode] = Decoder(net, max_rows=1, max_tokens=len(ids) + 600)
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
    rows["forced"] = {"today": _forced(net, ids, today)}
    for mode in made_by:
        rows["forced"][mode] = _forced(net, ids, made_by[mode])
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
    """The engine's decoder on `ASKS` as four rows of one bucket (a call `vlm.ask` serves: up to 16 rows), 256 greedy
    tokens each, drafted and plain: each row held against Transformers' teacher-forced forward at its clear positions
    (`forced`), and the same row decoded alone (a bucket of one) held the same way, with where the two first differ."""
    from cozy_runtime.engine.decoder import Decoder, Request

    net = model.pipe.components["model"]
    prompts = [model.processor.tokenizer(a, add_special_tokens=False)["input_ids"] for a in ASKS]
    rows: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "prompt_tokens": [len(p) for p in prompts]}
    head = getattr(net, "mtp", None)
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del net.mtp
        try:
            decoder = Decoder(net, max_rows=4, max_tokens=max(len(p) for p in prompts) + 300)
            made = decoder(Request(prompts=prompts, max_new=256, stops=[])).tokens
            alone = [decoder(Request(prompts=[p], max_new=256, stops=[])).tokens[0] for p in prompts]
        finally:
            if head is not None:
                net.mtp = head
        rows[mode] = [
            {"bucket": _forced(net, p, made[i]), "alone": _forced(net, p, alone[i]),
             "same_as_alone": next((t for t, (x, y) in enumerate(zip(made[i], alone[i])) if x != y), None)}
            for i, p in enumerate(prompts)
        ]  # fmt: skip
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
