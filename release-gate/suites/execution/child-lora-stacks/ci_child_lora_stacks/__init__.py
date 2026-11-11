"""A child call's own LoRA stack per slot (cl-687): `compare` calls sdxl's public `generate`, a dependency's function,
once per stack, on top of the request's `--lora`. The gate cell reads each call's stack from the record."""

from __future__ import annotations

import time
from typing import cast

import msgspec
import sdxl
from sdxl import GenerateInput

from cozy_runtime.author import App, Context, Lora, ModelChoice, Outputs, Seed, call
from cozy_runtime.media import File

app = App()
BALLOON, IKEA = "proomptengineer/balloon-lora-sdxl", "ostris/ikea-lora-sdxl"


class Compare(msgspec.Struct):
    prompt: str
    seed: int
    steps: int = 20


class Call(msgspec.Struct):
    stack: str
    seed: int
    seconds: float
    digest: str


class Compared(msgspec.Struct):
    calls: list[Call]
    #: Each distinct child image, in call order, as this run's own output (a list output).
    images: list[File]


async def compare(ctx: Context, payload: Compare, out: Outputs) -> Compared:
    s = payload.seed
    plan = [
        ("inherited", None, s),
        ("balloon 0 (dropped)", ModelChoice(lora=(Lora(BALLOON, 0),)), s),
        ("+ ikea 0.8", ModelChoice(lora=(Lora(IKEA, 0.8),)), s),
        ("balloon 0.5 (override)", ModelChoice(lora=(Lora(BALLOON, 0.5),)), s),
        ("inherited again, next seed", None, s + 1),
        ("inherited, first request again", None, s),
    ]
    calls: list[Call] = []
    images: list[File] = []
    for stack, model, seed in plan:
        started = time.monotonic()
        request = GenerateInput(prompt=payload.prompt, seed=cast(Seed, seed), steps=payload.steps)  # cast: Seed is a NewType of int
        result = await call(sdxl.generate, payload=request, model=model)
        seconds, digest = round(time.monotonic() - started, 2), result.image.digest
        seen = any(call.digest == digest for call in calls)
        if not seen:
            images.append(out.save_bytes(result.image.read_bytes(), media_type="image/webp"))
        calls.append(Call(stack, seed, seconds, digest))
    return Compared(calls, images)


app.entrypoint(compare, accelerator=False)
