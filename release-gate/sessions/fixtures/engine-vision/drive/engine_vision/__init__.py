"""engine-vision's drives (xs-213): CozyEngine's pictures and videos, through the Runtime as a run loads a lane.
- `inputs`: each case's prompt as the engine reads it (`CozyEngine._read`, `_encoded`) through the language model's own
  forward, whole and in 64-token chunks, against Transformers' forward of the processor's inputs: the last position's
  logits, and the rope delta. Runs on a CPU.
- `pictures`: the engine's decoder on the four cases as one bucket and each alone, drafted and plain, every row held
  against Transformers' teacher-forced forward with the pixel values at its clear positions; tokens kept a step.
- `caption`: Caption's question about a picture through CozyEngine and through Transformers' engine on one executor,
  warm, the same seeds: each call's wall time and tokens.
- `bucket`: the four cases as one call through both engines on one executor, warm: each call's wall time.
- `ask`: one case through `VisionLM.ask` (the run's `.engine` note says which engine served it)."""

import json
import time
from collections.abc import Callable

import msgspec
import torch
from cozy_runtime.author import App, Context
from cozy_runtime.media import ImageFile
from cozy_runtime.vlm import Params, Question, Turn, VisionLM, Watched
from PIL import Image

app = App()

#: qwen3's Caption question (packages qwen3 `_CAPTION`), before the picture.
CAPTION = (
    "Describe what this shows in detail, in one paragraph of at most 120 words: the subjects and every visible element, "
    "any visible text exactly as written, the materials, colours and lighting, the medium and style (for example photo, "
    "3D render, flat cel-shaded art, watercolour, woodblock print), the layout and the background; for a video, also "
    "what happens and how the camera moves. Describe only what you can see and invent nothing. Do not mention blur, "
    "noise, compression, resolution or image quality."
)


class Agreement(msgspec.Struct, frozen=True):
    detail: str


class Pictured(msgspec.Struct):
    #: the photograph the picture cases show
    image: ImageFile
    #: `ask`'s case; `caption`'s timed calls an engine
    case: str = "picture"
    runs: int = 3


def _video() -> Watched:
    """Four frames of a red square moving left to right over blue, half a second apart."""
    frames = []
    for i in range(4):
        frame = Image.new("RGB", (256, 256), (30, 60, 200))
        frame.paste((220, 30, 30), (20 + 60 * i, 100, 80 + 60 * i, 160))
        frames.append(frame)
    return Watched(frames, [0.25 + 0.5 * i for i in range(4)])


#: About a thousand tokens of plain English: a text prompt as long as a picture's (the picture's 1024 tokens and its words).
LONG = " ".join([
    "The lighthouse stood at the end of a long stone pier, its white tower streaked with rust where the iron rail met the "
    "wall. Every evening the keeper climbed the spiral stairs, counted the steps out of habit, and wiped the salt from the "
    "great lens before lighting the lamp. Ships passing in the dark never saw him, only the slow sweep of the beam across "
    "the water, but the fishermen in the harbour town said they could set their clocks by it."
] * 11)


def _cases(photo: Image.Image, long: bool = False) -> dict[str, Question]:
    """A picture; a picture in an earlier turn; a short video; text alone (with `long`, about as long as the picture's)."""
    text = f"{LONG}\n\nSay in two sentences what this passage describes." if long else "Name three colours and say one thing about each."
    return {
        "picture": Question(parts=[photo, "Describe this picture in two sentences."]),
        "earlier": Question(text="What was in the photograph? Answer in two sentences.", turns=[
            Turn("user", [photo, "Remember this photograph."]), Turn("assistant", ["I will."]),
        ]),  # fmt: skip
        "video": Question(parts=[_video(), "What happens in this video?"]),
        "text": Question(text=text),
    }


def _params(max_new: int, temperature: float = 0.0, seed: int = 0) -> Params:
    sampled = temperature > 0
    return Params(system="", thinking=False, reasoning_effort=None, stop=[], max_new_tokens=max_new,
                  temperature=temperature, top_p=0.8 if sampled else 1.0, presence_penalty=1.5 if sampled else 0.0,
                  seed=seed, top_k=20 if sampled else 0)  # fmt: skip


def _media_kwargs(media: object, extra: int) -> dict[str, torch.Tensor]:
    """The processor's media inputs for Transformers' forward, the token kinds grown by `extra` text tokens."""
    if media is None:
        return {}
    kinds = media.kinds  # type: ignore[attr-defined]  # engine.vision.Media
    kinds = torch.cat([kinds, torch.zeros((1, extra), dtype=kinds.dtype, device=kinds.device)], 1)
    named = {"pixel_values": media.pixels, "image_grid_thw": media.grid,  # type: ignore[attr-defined]  # as above
             "pixel_values_videos": media.video_pixels, "video_grid_thw": media.video_grid}  # type: ignore[attr-defined]
    return {"mm_token_type_ids": kinds, **{k: v for k, v in named.items() if v is not None}}


#: A position is clear where both Transformers reads lead by this much with the same top (`_forced`).
CLEAR = 0.5


#: Transformers' cached decodes by (prompt, tokens): a bucket's row and the same row alone often made the same tokens.
_DECODED: dict[tuple[tuple[int, ...], tuple[int, ...]], torch.Tensor] = {}


def _forced(net: torch.nn.Module, prompt: list[int], made: list[int], media: object) -> dict[str, object]:
    """`made` held against Transformers' teacher-forced reference, the prompt's pictures and videos in it: at each clear
    position, whether `made` took its choice. Clear, as the decode drive's: Transformers' forward over the prompt and
    `made` in one pass and its own cached decode of the same tokens, one at a time, both lead by at least `CLEAR` with the
    same top (`forward_clear`, `forward_agreed`: the forward's alone; `split`: both lead but choose apart)."""
    ids = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        logits = net(input_ids=ids, logits_to_keep=len(made), **_media_kwargs(media, len(made) - 1)).logits[0].float()
    top = logits.topk(2, dim=-1)
    choice, lead = top.indices[:, 0].tolist(), (top.values[:, 0] - top.values[:, 1]).tolist()
    gap = (top.values[:, 0] - logits.gather(1, torch.tensor(made, device=logits.device)[:, None])[:, 0]).tolist()
    key = (tuple(prompt), tuple(made))
    if key not in _DECODED:
        _DECODED[key] = _decoded(net, prompt, made, media)
    decoded = _DECODED[key]
    d_choice, d_lead, d_gap = (decoded[:, k].tolist() for k in range(3))
    forward = [t for t in range(len(made)) if lead[t] >= CLEAR]
    both = [t for t in forward if d_lead[t] >= CLEAR]
    clear = [t for t in both if int(d_choice[t]) == choice[t]]
    wrong = [t for t in clear if choice[t] != made[t]]
    misses = [t for t in forward if choice[t] != made[t]]
    found: dict[str, object] = {
        "clear": len(clear), "agreed": len(clear) - len(wrong), "first_wrong": wrong[0] if wrong else None,
        "forward_clear": len(forward), "forward_agreed": len(forward) - len(misses),
        "split": [t for t in both if int(d_choice[t]) != choice[t]][:8], "lead": [round(x, 3) for x in lead[:8]],
    }  # fmt: skip
    if misses:  # where the forward alone leads and `made` took another token: both reads there
        found["misses"] = [{"at": t, "lead": round(lead[t], 3), "gap": round(gap[t], 3), "decoded_lead": round(d_lead[t], 3),
                            "decoded_gap": round(d_gap[t], 3), "decoded_takes_made": int(d_choice[t]) == made[t]}
                           for t in misses[:8]]  # fmt: skip
    return found


def _kv(cache: object) -> dict[int, tuple[torch.Tensor, torch.Tensor]]:
    """A Transformers cache's attention layers' keys and values, by layer."""
    found = {}
    for i, layer in enumerate(getattr(cache, "layers", ())):
        keys, values = getattr(layer, "keys", None), getattr(layer, "values", None)
        if isinstance(keys, torch.Tensor) and isinstance(values, torch.Tensor) and keys.numel():
            found[i] = (keys, values)
    return found


def _decoded(net: torch.nn.Module, prompt: list[int], made: list[int], media: object) -> torch.Tensor:
    """Transformers' cached decode of `prompt` (its pictures and videos too) then `made`, a token at a time: at each
    position its top choice, its lead over the next, and how far `made`'s token sits below its top, `[len(made), 3]`. Its
    attention layers' K/V are held in `author.retained` between steps, as the decode drive's (44654637279: as plain
    tensors the Runtime's memory reused them under the cache, and the decode read 19 logits off its forward)."""
    from cozy_runtime.author import retained

    rows: list[torch.Tensor] = []
    stores: list[object] = []
    try:
        with torch.inference_mode():
            out = net(input_ids=torch.tensor([prompt], device=net.device), use_cache=True, logits_to_keep=1,
                      **_media_kwargs(media, 0))  # fmt: skip
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


def _apart(a: torch.Tensor, b: torch.Tensor) -> dict[str, object]:
    """Two reads of the same logits (or features): their largest and mean difference, and whether their tops agree."""
    d = (a.float() - b.float()).abs()
    return {"max": round(float(d.max()), 4), "mean": round(float(d.mean()), 5), "top_same": int(a.argmax()) == int(b.argmax())}


@app.entrypoint()
def inputs(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """Each case's prompt (a picture, one in an earlier turn, a video, and text about as long as the picture's) as
    CozyEngine reads and encodes it, through the language model's own forward, against Transformers' forward of the same
    processor inputs at the prompt's last position (`reference`: its head on that row alone, as ours), each as max and
    mean logit difference and top agreement:
    - `ours`: the engine's prefill (embeddings, positions `[4, 1, L]`), whole; `ours_3`: the same with the three M-RoPE
      rows alone, as Transformers' forward passes them; `chunked`: ours 64 tokens at a time with Transformers' cache;
    - Transformers' own spread: `reference_all` (its head on every row, as the earlier check read it) and `eager` (its
      forward with eager attention in place of its own, language model and vision tower alike).
    `features`: the vision tower's output in the engine's encode against Transformers' forward's (a hook on the tower),
    and under eager attention; `tower`: each call's attention implementation and dtypes. Also the rope delta."""
    from cozy_runtime.author import retained
    from transformers import DynamicCache

    net, engine = model.pipe.components["model"], model.engine
    visual = net.model.visual
    calls: list[dict[str, object]] = []
    outs: list[torch.Tensor] = []

    def seen(module: torch.nn.Module, args: tuple[object, ...], out: object) -> None:
        pooled = getattr(out, "pooler_output", None)
        pooled = torch.cat(list(pooled)) if isinstance(pooled, (tuple, list)) else pooled
        first = args[0] if args else None
        calls.append({"attn": str(getattr(module.config, "_attn_implementation", "?")),
                      "pixels": str(getattr(first, "dtype", "?")), "weights": str(next(module.parameters()).dtype),
                      "out": str(getattr(pooled, "dtype", "?"))})  # fmt: skip
        if isinstance(pooled, torch.Tensor):
            outs.append(pooled.detach().clone())

    handle = visual.register_forward_hook(seen)
    implementation = str(net.config._attn_implementation)
    rows: dict[str, object] = {"engine": type(engine).__name__, "device": str(net.device), "attn": implementation,
                               "text_attn": str(net.config.get_text_config(decoder=True)._attn_implementation)}  # fmt: skip
    try:
        for case, question in _cases(payload.image.read(), long=True).items():
            prompts, media = engine._read([question], _params(8))  # type: ignore[attr-defined]  # CozyEngine's
            (prompt,), (row,) = prompts, media
            ids = torch.tensor([prompt], device=net.device)
            outs.clear()
            calls.clear()
            with torch.inference_mode():
                if row is not None:
                    (prompted,) = engine._encoded(prompts, media)  # type: ignore[attr-defined]  # as above
                    embeds, positions, delta = prompted.embeds, prompted.positions, prompted.delta
                else:  # text: the token embeddings at text positions, as a text prompt's prefill reads them
                    embeds = net.get_input_embeddings()(ids)[0]
                    positions, delta = torch.arange(len(prompt), device=net.device).expand(3, -1), 0
                encoded = list(outs)
                kwargs = _media_kwargs(row, 0)
                reference = net(input_ids=ids, logits_to_keep=1, **kwargs).logits[0, -1]
                theirs = int(net.model.rope_deltas.reshape(-1)[0]) if row is not None else 0
                forwarded = list(outs[len(encoded):])
                reference_all = net(input_ids=ids, **kwargs).logits[0, -1]
                text = torch.arange(len(prompt), device=net.device)[None]
                at = torch.cat([text, positions])[:, None]
                decoder, head = net.get_decoder(), net.lm_head
                ours = head(decoder(inputs_embeds=embeds[None], position_ids=at).last_hidden_state[0, -1:])[0]
                ours_3 = head(decoder(inputs_embeds=embeds[None], position_ids=positions[:, None]).last_hidden_state[0, -1:])[0]
                cache = DynamicCache(config=net.config.get_text_config(decoder=True))
                stores: list[object] = []
                try:  # the cache's attention K/V held in `author.retained` between chunks, as `_decoded`'s
                    for c0 in range(0, len(prompt), 64):
                        y = decoder(inputs_embeds=embeds[None, c0 : c0 + 64], position_ids=at[..., c0 : c0 + 64],
                                    past_key_values=cache, use_cache=True).last_hidden_state  # fmt: skip
                        held = _kv(cache)
                        store = retained(sum(k.nbytes + v.nbytes for k, v in held.values()))
                        for layer, (k, v) in held.items():
                            cache.layers[layer].keys, cache.layers[layer].values = store.put(k, v).get()
                        del held
                        stores.append(store)
                        if len(stores) > 1:
                            stores.pop(0).close()  # type: ignore[attr-defined]  # a Store
                    chunked = head(y[0, -1:])[0]
                finally:
                    for store in stores:
                        store.close()  # type: ignore[attr-defined]  # a Store
                eager: dict[str, object] = {}
                try:
                    net.set_attn_implementation("eager")
                    before = len(outs)
                    eager_logits = net(input_ids=ids, logits_to_keep=1, **kwargs).logits[0, -1]
                    eager = _apart(eager_logits, reference)
                    if row is not None and len(outs) > before and forwarded:
                        eager["features"] = _apart(outs[before], forwarded[0])
                except Exception as exc:  # told, and the rest stands
                    eager = {"error": f"{type(exc).__name__}: {exc}"[:300]}
                finally:
                    net.set_attn_implementation(implementation)
            found: dict[str, object] = {
                "tokens": len(prompt), "delta": delta, "their_delta": theirs,
                "ours": _apart(ours, reference), "ours_3": _apart(ours_3, reference), "chunked": _apart(chunked, reference),
                "reference_all": _apart(reference_all, reference), "eager": eager,
            }  # fmt: skip
            if row is not None:
                found["media_tokens"] = int((row.kinds != 0).sum())
                kinds = row.kinds[0] != 0
                if encoded and forwarded:
                    found["features"] = _apart(torch.cat(encoded), torch.cat(forwarded))
                    found["features_in_embeds"] = _apart(embeds[kinds], torch.cat(forwarded).to(embeds.dtype))
                found["tower"] = list(calls)
            rows[case] = found
    finally:
        handle.remove()
    return Agreement(json.dumps(rows))


@app.entrypoint(accelerator=True)
def pictures(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """The engine's decoder (the model's declared kernel set, on the launch path the engine judged) on the four cases as
    one bucket and each alone, 192 greedy tokens, drafted and plain: each row held against Transformers' teacher-forced
    forward at its clear positions, with where the bucket's and the alone's tokens first differ, and tokens kept a step."""
    from cozy_runtime.engine import launcher
    from cozy_runtime.engine.decoder import Decoder, Request
    from cozy_runtime.engine.launch import JIT

    net, engine = model.pipe.components["model"], model.engine
    index = int(getattr(engine, "index", 0))
    deadline = time.monotonic() + 300
    while (paths := launcher.judged(index)) is None:
        if time.monotonic() > deadline:
            raise RuntimeError("the engine's launcher check never ended")
        time.sleep(1)
    if paths.launcher and paths.replayed:
        raise RuntimeError(f"this driver runs neither launch path: {paths.launcher}; {paths.replayed}")
    cases = _cases(payload.image.read(), long=True)
    names = list(cases)
    read = [engine._read([q], _params(8)) for q in cases.values()]  # type: ignore[attr-defined]  # CozyEngine's
    prompts = [p for (ps, _) in read for p in ps]
    media = [m for (_, ms) in read for m in ms]
    run = getattr(engine, "run", None)
    rows: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "declared": run is not None,
                               "launcher": not paths.launcher, "tokens": dict(zip(names, map(len, prompts), strict=True))}  # fmt: skip
    from cozy_runtime import lm

    # Today's path on each case alone (Transformers' engine's `lm.generate`, drafted as a call is), held the same way.
    today = {}
    for i, (name, question) in enumerate(cases.items()):
        made = lm.generate(net, engine.fallback.inputs([question], _params(8)), max_new_tokens=192, stop=[], pad=0)  # type: ignore[attr-defined]  # CozyEngine's
        today[name] = _forced(net, prompts[i], [int(t) for t in made[0].tolist()], media[i])
    rows["today"] = today
    head = getattr(net, "mtp", None)
    for mode in ("drafted", "plain") if head is not None else ("plain",):
        if mode == "plain" and head is not None:
            del net.mtp
        try:
            decoder = Decoder(net, max_rows=4, max_tokens=max(map(len, prompts)) + 300, run=run or JIT,
                              launcher=not paths.launcher)  # fmt: skip
            prompted = engine._encoded(prompts, media)  # type: ignore[attr-defined]  # as above
            rows["deltas"] = dict(zip(names, [None if p is None else p.delta for p in prompted], strict=True))
            bucket = decoder(Request(prompts=prompts, max_new=192, stops=[], prompted=prompted))
            alone = [decoder(Request(prompts=[p], max_new=192, stops=[], prompted=[g])) for p, g in zip(prompts, prompted, strict=True)]
            del prompted
        finally:
            if head is not None:
                net.mtp = head
        rows[mode] = {
            name: {"bucket": _forced(net, prompts[i], bucket.tokens[i], media[i]),
                   "alone": _forced(net, prompts[i], alone[i].tokens[0], media[i]),
                   "same_as_alone": next((t for t, (x, y) in enumerate(zip(bucket.tokens[i], alone[i].tokens[0])) if x != y), None),
                   "kept_a_step": round(sum(alone[i].kept[0]) / max(len(alone[i].kept[0]), 1), 3),
                   "bucket_kept_a_step": round(sum(bucket.kept[i]) / max(len(bucket.kept[i]), 1), 3),
                   "captured": alone[i].captured, "notes": alone[i].notes}
            for i, name in enumerate(names)
        }  # fmt: skip
    return Agreement(json.dumps(rows))


#: Untimed calls each engine makes first: the second still recorded graphs Transformers' engine had not (44639542665).
WARM = 2


def _both(model: VisionLM, questions: list[Question], params: Callable[[int], Params], runs: int) -> dict[str, object]:
    """`questions` through CozyEngine and through Transformers' engine on this executor: each warmed by `WARM` calls,
    then `runs` timed calls each, the two arms alternating which goes first, both at seed k on call k. CozyEngine must
    serve each itself (its `Final` carries the engine's timings). `call_x`: Transformers' mean call over the engine's;
    `token_x`: its seconds a token made over the engine's."""
    engine = model.engine
    arms = {"engine": engine, "transformers": engine.fallback}  # type: ignore[attr-defined]  # CozyEngine's
    if why := engine._why(questions, params(0)):  # type: ignore[attr-defined]  # CozyEngine's routing
        raise RuntimeError(f"CozyEngine doesn't serve these: {why}")

    def call(name: str, seed: int) -> dict[str, object]:
        torch.cuda.synchronize()
        began = time.perf_counter()
        final = arms[name].generate(questions, params(seed), on_delta=lambda _d: None)
        torch.cuda.synchronize()
        seconds = time.perf_counter() - began
        if name == "engine" and "decode" not in final.timings:
            raise RuntimeError(f"seed {seed}: CozyEngine handed the call on (no engine timings)")
        return {"seed": seed, "s": round(seconds, 3), "tokens": final.usage.completion,
                "prefill_s": round(final.timings.get("prefill", 0.0), 3), "decode_s": round(final.timings.get("decode", 0.0), 3),
                "words": final.texts[0][:200]}  # fmt: skip

    for k in range(WARM):
        for name in arms:
            call(name, 1000 + k)
    timed: dict[str, list[dict[str, object]]] = {name: [] for name in arms}
    for k in range(1, runs + 1):
        for name in (("engine", "transformers") if k % 2 else ("transformers", "engine")):
            timed[name].append(call(name, k))
    seconds = {n: sum(float(c["s"]) for c in calls) for n, calls in timed.items()}  # type: ignore[arg-type]
    tokens = {n: sum(int(c["tokens"]) for c in calls) for n, calls in timed.items()}  # type: ignore[call-overload]
    return {"gpu": torch.cuda.get_device_name(), "warm": WARM, **timed,
            "call_x": round(seconds["transformers"] / seconds["engine"], 3),
            "token_x": round((seconds["transformers"] / max(tokens["transformers"], 1)) / (seconds["engine"] / max(tokens["engine"], 1)), 3)}  # fmt: skip


@app.entrypoint(accelerator=True)
def caption(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """Caption's question about the picture (its sampling: Qwen's card, top-k 20, presence 1.5) through both engines,
    warm, alternating (`_both`)."""
    questions = [Question(parts=[f"{CAPTION}\n\n", payload.image.read()])]
    return Agreement(json.dumps(_both(model, questions, lambda seed: _params(1024, 0.7, seed), payload.runs)))


@app.entrypoint(accelerator=True)
def bucket(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """The four cases as one call (a picture, a picture in an earlier turn, a video, text), greedy, at most 256 tokens a
    row, through both engines, warm, alternating (`_both`). Transformers' engine reads the rows' pictures and videos in
    one vision pass, CozyEngine a row at a time; the rows' lengths may differ, so `token_x` is the fair figure."""
    questions = list(_cases(payload.image.read()).values())
    return Agreement(json.dumps(_both(model, questions, lambda seed: _params(256, 0.0, seed), payload.runs)))


@app.entrypoint(accelerator=True)
def ask(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """One case through `VisionLM.ask`, greedy, 48 tokens: its answer; the run's `.engine` note says which engine served it."""
    question = _cases(payload.image.read())[payload.case]
    (answer,) = model.ask([question], system="", thinking=False, stop=[], max_new_tokens=48, presence_penalty=0.0,
                          temperature=0.0, top_p=1.0, seed=0)  # fmt: skip
    if not answer.strip():
        raise RuntimeError(f"{payload.case}: no answer")
    return Agreement(json.dumps({"case": payload.case, "answer": answer[:200]}))
