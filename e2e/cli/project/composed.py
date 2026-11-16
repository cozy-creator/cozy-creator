# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A still that reads a fine-tune's transformer and a base's VAE as raw weights, and has a third slot it fetches only on demand and
never reads: what cl-1071's plan_fetch quotes, a run's download being what it fetches (e2e/composed-* in e2e/isolated.sh)."""

import os

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Seed, Weights
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Composed", named="composed",
          defaults={"model": "e2e/composed-base@0.1.0", "vae": "e2e/composed-base@0.1.0", "extra": "e2e/composed-extra@0.1.0"},
          components={"model": ("transformer",), "vae": ("vae",)}, on_demand=("extra",))  # fmt: skip
async def composed(ctx: Context, payload: Words, model: Weights, vae: Weights, extra: Weights) -> ImageResult:
    """A still of one colour, made once its transformer and VAE are here; `extra` is never read."""
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
