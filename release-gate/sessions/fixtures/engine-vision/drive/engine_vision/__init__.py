"""engine-vision's drives (xs-213): CozyEngine's pictures and videos, through the Runtime as a run loads a lane.
- `inputs`: each case's prompt as the engine reads it (`CozyEngine._read`, `_encoded`) through the language model's own
  forward, whole and in 64-token chunks, against Transformers' forward of the processor's inputs: the last position's
  logits, and the rope delta. Runs on a CPU.
- `pictures`: the engine's decoder on the four cases as one bucket and each alone, drafted and plain, every row held
  against Transformers' teacher-forced forward with the pixel values at its clear positions; tokens kept a step.
- `caption`: Caption's question about a picture through CozyEngine and through Transformers' engine on one executor,
  warm, the same seeds: each call's wall time and tokens.
- `ask`: one case through `VisionLM.ask` (the run's `.engine` note says which engine served it)."""

import json
import time

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


def _cases(photo: Image.Image) -> dict[str, Question]:
    """A picture; a picture in an earlier turn; a short video; text alone."""
    return {
        "picture": Question(parts=[photo, "Describe this picture in two sentences."]),
        "earlier": Question(text="What was in the photograph? Answer in two sentences.", turns=[
            Turn("user", [photo, "Remember this photograph."]), Turn("assistant", ["I will."]),
        ]),  # fmt: skip
        "video": Question(parts=[_video(), "What happens in this video?"]),
        "text": Question(text="Name three colours and say one thing about each."),
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


def _forced(net: torch.nn.Module, prompt: list[int], made: list[int], media: object) -> dict[str, object]:
    """`made` held against Transformers' own forward over the prompt (its pictures and videos too) and `made` in one pass
    (teacher-forced): at each position whose top logit leads the next by at least 0.5, whether `made` took it."""
    ids = torch.tensor([prompt + made[:-1]], device=net.device)
    with torch.inference_mode():
        logits = net(input_ids=ids, logits_to_keep=len(made), **_media_kwargs(media, len(made) - 1)).logits[0].float()
    top = logits.topk(2, dim=-1)
    choice, lead = top.indices[:, 0].tolist(), (top.values[:, 0] - top.values[:, 1]).tolist()
    clear = [t for t in range(len(made)) if lead[t] >= 0.5]
    wrong = [t for t in clear if choice[t] != made[t]]
    return {"clear": len(clear), "agreed": len(clear) - len(wrong), "first_wrong": wrong[0] if wrong else None,
            "lead": [round(x, 3) for x in lead[:8]]}


@app.entrypoint()
def inputs(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """Each case's prompt as CozyEngine reads and encodes it, through the language model's own forward (whole, and 64
    tokens at a time with Transformers' cache), against Transformers' forward of the same processor inputs: the largest
    difference of the last position's logits, whether their top tokens agree, and the rope delta against Transformers'."""
    from transformers import DynamicCache

    net, engine = model.pipe.components["model"], model.engine
    rows: dict[str, object] = {"engine": type(engine).__name__, "device": str(net.device)}
    for case, question in _cases(payload.image.read()).items():
        prompts, media = engine._read([question], _params(8))  # type: ignore[attr-defined]  # CozyEngine's
        (prompt,), (row,) = prompts, media
        if row is None:
            rows[case] = {"tokens": len(prompt), "media": False}
            continue
        (prompted,) = engine._encoded(prompts, media)  # type: ignore[attr-defined]  # as above
        ids = torch.tensor([prompt], device=net.device)
        with torch.inference_mode():
            reference = net(input_ids=ids, **_media_kwargs(row, 0)).logits[0, -1].float()
            theirs = int(net.model.rope_deltas.reshape(-1)[0])
            text = torch.arange(len(prompt), device=net.device)[None]
            at = torch.cat([text, prompted.positions])[:, None]
            whole = net.lm_head(net.get_decoder()(inputs_embeds=prompted.embeds[None], position_ids=at).last_hidden_state[0, -1:])[0].float()
            cache = DynamicCache(config=net.config.get_text_config(decoder=True))
            for c0 in range(0, len(prompt), 64):
                y = net.get_decoder()(inputs_embeds=prompted.embeds[None, c0 : c0 + 64], position_ids=at[..., c0 : c0 + 64],
                                      past_key_values=cache, use_cache=True).last_hidden_state  # fmt: skip
            chunked = net.lm_head(y[0, -1:])[0].float()
        rows[case] = {
            "tokens": len(prompt), "media_tokens": int((row.kinds != 0).sum()), "delta": prompted.delta, "their_delta": theirs,
            "whole_max_diff": float((whole - reference).abs().max()), "whole_top_same": int(whole.argmax()) == int(reference.argmax()),
            "chunked_max_diff": float((chunked - reference).abs().max()), "chunked_top_same": int(chunked.argmax()) == int(reference.argmax()),
        }  # fmt: skip
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
    cases = _cases(payload.image.read())
    names = list(cases)
    read = [engine._read([q], _params(8)) for q in cases.values()]  # type: ignore[attr-defined]  # CozyEngine's
    prompts = [p for (ps, _) in read for p in ps]
    media = [m for (_, ms) in read for m in ms]
    run = getattr(engine, "run", None)
    rows: dict[str, object] = {"gpu": torch.cuda.get_device_name(), "declared": run is not None,
                               "launcher": not paths.launcher, "tokens": dict(zip(names, map(len, prompts), strict=True))}  # fmt: skip
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
                   "captured": alone[i].captured, "notes": alone[i].notes}
            for i, name in enumerate(names)
        }  # fmt: skip
    return Agreement(json.dumps(rows))


@app.entrypoint(accelerator=True)
def caption(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """Caption's question about the picture (its sampling: Qwen's card, top-k 20, presence 1.5) through CozyEngine and
    through Transformers' engine on this executor: each warmed by one call, then `runs` calls each at seeds 1 to `runs`,
    timed to their end. CozyEngine must serve each itself (its `Final` carries the engine's timings)."""
    engine = model.engine
    fallback = engine.fallback  # type: ignore[attr-defined]  # CozyEngine's Transformers' engine
    questions = [Question(parts=[f"{CAPTION}\n\n", payload.image.read()])]
    if why := engine._why(questions, _params(1024, 0.7)):  # type: ignore[attr-defined]  # CozyEngine's routing
        raise RuntimeError(f"CozyEngine doesn't serve Caption: {why}")
    timed: dict[str, list[dict[str, object]]] = {}
    seconds_of: dict[str, float] = {}
    ms_of: dict[str, float] = {}
    for name, serving in (("engine", engine), ("transformers", fallback)):
        calls: list[dict[str, object]] = []
        total = per = 0.0
        for seed in range(payload.runs + 1):
            torch.cuda.synchronize()
            began = time.perf_counter()
            final = serving.generate(questions, _params(1024, 0.7, seed), on_delta=lambda _d: None)
            torch.cuda.synchronize()
            seconds = time.perf_counter() - began
            if name == "engine" and "decode" not in final.timings:
                raise RuntimeError(f"seed {seed}: CozyEngine handed Caption on (no engine timings)")
            if seed:  # seed 0 warms
                ms = 1e3 * seconds / max(final.usage.completion, 1)
                total, per = total + seconds, per + ms
                calls.append({"seed": seed, "s": round(seconds, 3), "tokens": final.usage.completion, "ms_token": round(ms, 2),
                              "prefill_s": round(final.timings.get("prefill", 0.0), 3), "words": final.texts[0][:200]})  # fmt: skip
        timed[name], seconds_of[name], ms_of[name] = calls, total / payload.runs, per / payload.runs
    return Agreement(json.dumps({
        "gpu": torch.cuda.get_device_name(), **timed,
        "call_x": round(seconds_of["transformers"] / seconds_of["engine"], 3),
        "token_x": round(ms_of["transformers"] / ms_of["engine"], 3),
    }))  # fmt: skip


@app.entrypoint(accelerator=True)
def ask(ctx: Context, payload: Pictured, model: VisionLM) -> Agreement:
    """One case through `VisionLM.ask`, greedy, 48 tokens: its answer; the run's `.engine` note says which engine served it."""
    question = _cases(payload.image.read())[payload.case]
    (answer,) = model.ask([question], system="", thinking=False, stop=[], max_new_tokens=48, presence_penalty=0.0,
                          temperature=0.0, top_p=1.0, seed=0)  # fmt: skip
    if not answer.strip():
        raise RuntimeError(f"{payload.case}: no answer")
    return Agreement(json.dumps({"case": payload.case, "answer": answer[:200]}))
