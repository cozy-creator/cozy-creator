"""The tools cells' package (design/tools.md, weightless 2, 5 and 6). `composed` calls `e2e-probe-callee`'s `shade`, branches on what it
says, calls its own `tint` when the shade came out dark, publishes twice, and enlarges only when asked. `fails_until` names a file on its
machine: until it's there, the tool fails after shading, so asking again finds the shade memoized. `hold` keeps the shade going, to be
canceled mid-step."""

import os
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Outputs, Seed, call
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile
from e2e_probe_callee import EnlargeInput, ShadeInput, enlarge, shade

app = App()
SIDE = 16


def _still(out: Outputs, level: int) -> ImageFile:
    return out.save_image(ImageFrame(SIDE, SIDE, bytes([level]) * 3 * SIDE**2), format="png")


class Tinted(msgspec.Struct):
    image: ImageFile


@app.entrypoint(private=True, accelerator=False)
async def tint(ctx: Context, *, image: ImageFile, out: Outputs) -> Tinted:
    """A lighter still, for a page that came out dark."""
    return Tinted(_still(out, 200))


class Compose(msgspec.Struct, kw_only=True):
    page: ImageFile
    instructions: Prompt[Image] = Prompt()
    darkness: Annotated[float, Range(0.0, 1.0, step=0.25)] = 0.75
    enlarge: bool = False
    hold: Annotated[float, Range(0, 60, unit="s")] = 0
    fails_until: str = ""
    seed: Seed | None = None


class Composed(msgspec.Struct):
    image: ImageFile
    warnings: list[str] = []


class NotYet(Exception):
    """The deliberate failure: what it waits for isn't there yet."""


@app.tool(label="Probe: composed", named="composed", accelerator=False)
async def composed(ctx: Context, payload: Compose, out: Outputs) -> Composed:
    """Shade the page in the other package; lighten it here if that came out dark; enlarge it if asked."""
    shaded = await call(shade, payload=ShadeInput(image=payload.page, amount=payload.darkness, hold=payload.hold))
    out.publish("image", shaded.image, label="Shaded")
    if payload.fails_until and not os.path.exists(payload.fails_until):
        raise NotYet(f"{payload.fails_until} isn't there yet")
    page = shaded.image
    if shaded.dark:
        page = (await call(tint, image=page)).image
        out.publish("image", page, label="Lightened")
    if payload.enlarge:
        page = (await call(enlarge, payload=EnlargeInput(image=page))).image
    return Composed(page)
