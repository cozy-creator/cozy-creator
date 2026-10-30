"""A child call's own LoRA stack per slot (cl-687): `compare` calls `paint` (sdxl's own generate) once per stack,
on top of the request's `--lora`. The gate cell reads each call's stack from the record."""

from __future__ import annotations

import time

import msgspec
import sdxl
from sdxl import GenerateInput, SdxlModel

from cozy_runtime.author import (
    Adjustments,
    App,
    Context,
    FileAsset,
    ImageResult,
    Lora,
    ModelChoice,
    Outputs,
    Telemetry,
    invocable,
)

app = App()
BALLOON, IKEA = "proomptengineer/balloon-lora-sdxl", "ostris/ikea-lora-sdxl"


@invocable(defaults={"model": "stabilityai/sdxl@1.0.0"}, memoize=True)
async def paint(
    ctx: Context, *, prompt: str, seed: int, steps: int = 20, model: SdxlModel, out: Outputs,
    tel: Telemetry, adj: Adjustments,
) -> ImageResult:
    """sdxl's own generate, as this package's GPU call."""
    payload = GenerateInput(prompt=prompt, seed=seed, steps=steps)
    generate = sdxl.app._registry["generate"].fn
    return generate(ctx, payload, sdxl.resolve_generate(payload), model, out, tel, adj)


app.entrypoint(internal=True)(paint)


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
    images: list[FileAsset]


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
    images: list[FileAsset] = []
    for stack, model, seed in plan:
        started = time.monotonic()
        result = await paint(prompt=payload.prompt, seed=seed, steps=payload.steps, model=model)
        seconds, digest = round(time.monotonic() - started, 2), result.image.digest
        seen = any(call.digest == digest for call in calls)
        if not seen:
            images.append(out.save_bytes(result.image.read_bytes(), media_type="image/webp"))
        calls.append(Call(stack, seed, seconds, digest))
    return Compared(calls, images)


app.entrypoint(compare, accelerator=False)
