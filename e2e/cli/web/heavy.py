# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The localfit cell's tool: a still made on the CPU by a tool that, as MiniMax H3's shot does, composes a GPU function of its own that
loads a model. That is what a computer with no GPU does poorly, so rendering it there asks first. The model paints the pictures its
words mention: a first render's none, a render again its take shown (cl-909). `seconds` holds it that long first: render-placement's runs
here are slow beside the rental's, so this computer's fit compares the two (cl-897)."""

import asyncio
import os

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Outputs, Seed, Weights, call
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Painted(msgspec.Struct):
    image: ImageFile


@app.entrypoint(private=True, defaults={"weights": "zhengpeng7/birefnet@0.1.0"})
async def paint(ctx: Context, *, image: ImageFile, weights: Weights, out: Outputs) -> Painted:
    """The model on a GPU that makes `heavy` one: a picture of its own, as a function returns only what it saved (foreign_handle)."""
    return Painted(out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"))


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    seconds: float = 0


@app.tool(label="Heavy", named="heavy", accelerator=False)
async def heavy(ctx: Context, payload: Words, out: Outputs) -> ImageResult:
    """A still of one colour, and each picture it mentions painted by the model."""
    await asyncio.sleep(payload.seconds)
    for picture in payload.instructions.references.values():
        await call(paint, image=picture.file)
    image = out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
