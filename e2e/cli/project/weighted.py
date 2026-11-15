# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A still that reads BiRefNet's raw weights and takes `seconds`: its machine downloads those weights before it runs, and as they are
built on no device it runs on a CPU too. cl-967's plan cell places it where they already are."""

import asyncio
import os

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Outputs, Seed, Weights
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult

app = App()
SIDE = 16


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    seconds: float = 0


@app.tool(label="Weighted", named="weighted", defaults={"weights": "zhengpeng7/birefnet@0.1.0"})
async def weighted(ctx: Context, payload: Words, weights: Weights, out: Outputs) -> ImageResult:
    """A still of one colour, made once BiRefNet's weights are here."""
    await asyncio.sleep(payload.seconds)
    image = out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
