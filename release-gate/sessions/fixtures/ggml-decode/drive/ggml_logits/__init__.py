"""ggml-decode's drives (xs-213), through the Runtime as a run loads a lane (its leaves, its kernels).
- `logits`: a lane's next-token distributions against a reference lane's on the same text. Per position: whether the
  top tokens agree, and KL(reference || lane) in nats.
- `decode`: the lane's single-row greedy decode of `PROMPT` for a fixed number of tokens, timed with the checkpoint's
  MTP head drafting (as every call runs) and without it, warm (each after its own warm-up).
- `rows`, `routes`: the engine-routes session's calls that `qwen3/ask` can't make.
- `quality`: one lane's top-1 next tokens and perplexity on `TEXTS`, compared across calls.
- `opening_kernel`: cr-342's cell through the Runtime's bench (cozy-runtime#2322), in this environment, which holds
  the Transformers it builds its small model with."""

import hashlib
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


#: A choice is clear where Transformers' one-pass forward and its own cached decode both lead by at least this.
CLEAR = 0.5


def _kv(cache: object) -> dict[int, tuple[torch.Tensor, torch.Tensor]]:
    """A Transformers cache's attention layers' keys and values, by layer."""
    found = {}
    for i, layer in enumerate(getattr(cache, "layers", ())):
        keys, values = getattr(layer, "keys", None), getattr(layer, "values", None)
        if isinstance(keys, torch.Tensor) and isinstance(values, torch.Tensor) and keys.numel():
            found[i] = (keys, values)
    return found


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
    lead over the next, and how far `made`'s token sits below its top, as `[len(made), 3]` floats. Its attention
    layers' K/V are held in `author.retained` between steps: as plain tensors the Runtime's memory reused them under
    the cache (44654637279: every attention layer's cached K/V gone, the decode 19 logits off its forward)."""
    from cozy_runtime.author import retained

    rows: list[torch.Tensor] = []
    stores: list[object] = []
    try:
        with torch.inference_mode():
            out = net(input_ids=torch.tensor([prompt], device=net.device), use_cache=True, logits_to_keep=1)
            for t, token in enumerate(made):
                last = out.logits[0, -1].float()
                v, i = last.topk(2)
                rows.append(torch.stack([i[0].float(), v[0] - v[1], v[0] - last[token]]))
                if t + 1 < len(made):
                    cache = out.past_key_values
                    held = _kv(cache)
                    store = retained(sum(k.nbytes + v.nbytes for k, v in held.values()))
                    for layer, (k, v) in held.items():
                        cache.layers[layer].keys, cache.layers[layer].values = store.put(k, v).get()
                    del held
                    stores.append(store)
                    if len(stores) > 1:
                        stores.pop(0).close()  # type: ignore[attr-defined]  # a Store
                    out = net(input_ids=torch.tensor([[token]], device=net.device), past_key_values=cache,
                              use_cache=True, logits_to_keep=1)  # fmt: skip
    finally:
        for store in stores:
            store.close()  # type: ignore[attr-defined]  # a Store
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
    from cozy_runtime.engine import launcher

    index = int(getattr(model.engine, "index", 0))
    began = time.monotonic()
    while (paths := launcher.judged(index)) is None:
        if time.monotonic() > began + 300:
            raise RuntimeError("the engine's launcher check never ended")
        time.sleep(1)
    if paths.launcher and paths.replayed:
        raise RuntimeError(f"this driver runs neither launch path: {paths.launcher}; {paths.replayed}")
    return _bucket(ctx, model, launcher=not paths.launcher, judged_s=time.monotonic() - began)


@app.entrypoint(accelerator=True)
def bucket_replayed(ctx: Context, model: VisionLM) -> Agreement:
    """`_bucket` with the host replaying the step's graph (`Decoder(launcher=False)`): no WHILE node."""
    return _bucket(ctx, model, launcher=False)


def _bucket(ctx: Context, model: VisionLM, launcher: bool, judged_s: float = 0.0) -> Agreement:
    """The engine's decoder on `ASKS` as four rows of one bucket (a call `vlm.ask` serves: up to 16 rows), 256 greedy
    tokens each, drafted and plain: each row held against Transformers' teacher-forced forward at its clear positions
    (`forced`), and the same row decoded alone (a bucket of one) held the same way, with where the two first differ.
    Its kernels are the model's declared set where CozyEngine installed one (`declared`), as a call's are. `s`: the wait
    for the launcher's verdict and each mode's wall seconds (the bucket, the rows alone, the teacher-forced reads), each
    stage also the run's progress."""
    from cozy_runtime.engine.decoder import Decoder, Request
    from cozy_runtime.engine.launch import JIT

    net = model.pipe.components["model"]
    prompts = [model.processor.tokenizer(a, add_special_tokens=False)["input_ids"] for a in ASKS]
    run = getattr(model.engine, "run", None)
    rows: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "prompt_tokens": [len(p) for p in prompts],
                               "declared": run is not None, "launcher": launcher}
    memo: dict[tuple[tuple[int, ...], tuple[int, ...]], torch.Tensor] = {}
    head = getattr(net, "mtp", None)
    modes = ("drafted", "plain") if head is not None else ("plain",)
    seconds: dict[str, object] = {"judged": round(judged_s, 1)}
    rows["s"] = seconds
    stage, stages = 0, len(modes) * (2 + len(prompts))

    def done() -> None:
        nonlocal stage
        ctx.progress(stage, stages)
        stage += 1

    for mode in modes:
        if mode == "plain" and head is not None:
            del net.mtp
        try:
            decoder = Decoder(net, max_rows=4, max_tokens=max(len(p) for p in prompts) + 300, run=run or JIT,
                              launcher=launcher)
            began = time.monotonic()
            made = decoder(Request(prompts=prompts, max_new=256, stops=[])).tokens
            torch.cuda.synchronize()
            took = {"bucket": round(time.monotonic() - began, 1)}
            done()
            began = time.monotonic()
            alone = [decoder(Request(prompts=[p], max_new=256, stops=[])).tokens[0] for p in prompts]
            torch.cuda.synchronize()
            took["alone"] = round(time.monotonic() - began, 1)
            done()
        finally:
            if head is not None:
                net.mtp = head
        began = time.monotonic()
        held = []
        for i, p in enumerate(prompts):
            held.append({"bucket": _forced(net, p, made[i], memo), "alone": _forced(net, p, alone[i], memo),
                         "same_as_alone": next((t for t, (x, y) in enumerate(zip(made[i], alone[i])) if x != y), None)})
            done()
        took["forced"] = round(time.monotonic() - began, 1)
        seconds[mode] = took
        rows[mode] = held
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
            with _HeadLinears(int(getattr(_output_head(net), "out_features"))) as seen:
                made = lm.generate(net, inputs, max_new_tokens=12, stop=[], pad=0, temperature=0.7, seed=213)
        finally:
            if head is not None:
                lm.HEADS[net] = head
        rows[mode] = {"tokens": int(made.shape[1]), "head_linears": seen.rows}
    return Agreement(json.dumps(rows))
