# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A still that reads a fine-tune's transformer and a base's VAE as raw weights, and has a third slot it fetches only on demand, read
only with `read_extra`: what cl-1071's plan_fetch quotes, a run's download being what it fetches (e2e/composed-* in e2e/isolated.sh)."""

import math
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
    #: Reads every byte of `extra`, which its machine then fetches inside the run (on demand).
    read_extra: bool = False


#: The most one `read_into` reads.
PIECE = 16 << 20


@app.tool(label="Composed", named="composed",
          defaults={"model": "e2e/composed-base@0.1.0", "vae": "e2e/composed-base@0.1.0", "extra": "e2e/composed-extra@0.1.0"},
          components={"model": ("transformer",), "vae": ("vae",)}, on_demand=("extra",))  # fmt: skip
async def composed(ctx: Context, payload: Words, model: Weights, vae: Weights, extra: Weights) -> ImageResult:
    """A still of one colour, made once its transformer and VAE are here; `extra` read whole with `read_extra`."""
    if payload.read_extra:
        piece = bytearray(PIECE)
        for component in extra.components():
            for key in extra.keys(component):
                size = math.prod(extra.tensor(component, key).shape) * 2  # F16, as e2e/isolated.sh's random_weights writes it
                for offset in range(0, size, PIECE):
                    extra.read_into(component, key, "", offset, memoryview(piece)[: min(PIECE, size - offset)])
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
