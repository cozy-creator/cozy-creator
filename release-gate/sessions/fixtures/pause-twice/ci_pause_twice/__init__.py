"""A deviceless root calling SDXL twice with a pause (xs-088 review R1): between the calls its GPU executor is
idle while the root runs. The machine must neither end it nor drop its recorded loop: the second call replays."""

from __future__ import annotations

import asyncio
import time

import msgspec
import sdxl
from sdxl import GenerateInput, SdxlModel

from cozy_runtime.author import Adjustments, App, Context, ImageResult, Outputs, Telemetry, invocable

app = App()


@invocable(defaults={"model": "stabilityai/sdxl@1.0.0"})
async def paint(
    ctx: Context, *, prompt: str, seed: int, steps: int = 20, model: SdxlModel, out: Outputs,
    tel: Telemetry, adj: Adjustments,
) -> ImageResult:
    """sdxl's own generate, as this package's GPU call."""
    payload = GenerateInput(prompt=prompt, seed=seed, steps=steps)
    generate = sdxl.app._registry["generate"].fn
    return generate(ctx, payload, sdxl.resolve_generate(payload), model, out, tel, adj)


app.entrypoint(internal=True)(paint)


class Twice(msgspec.Struct):
    prompt: str
    seed: int
    pause_s: float = 30.0


class Done(msgspec.Struct):
    seconds: list[float]


async def twice(ctx: Context, payload: Twice, out: Outputs) -> Done:
    seconds = []
    for i in range(2):
        if i:
            await asyncio.sleep(payload.pause_s)
        started = time.monotonic()
        await paint(prompt=payload.prompt, seed=payload.seed + i)
        seconds.append(round(time.monotonic() - started, 2))
    return Done(seconds)


app.entrypoint(twice, accelerator=False)
