"""The queue quote cell's tool (cl-1107, e2e/cli/plan.sh's plan_queue): a still once BiRefNet's weights are here, timed by its
benchmarks.json as H3's generate is (230 s on an RTX 5090, 335 s on an RTX 4090, 196 s on two)."""

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


@app.tool(label="Shot", named="shot", defaults={"weights": "zhengpeng7/birefnet@0.1.0"})
async def shot(ctx: Context, payload: Words, weights: Weights) -> ImageResult:
    """A still of one colour, made once BiRefNet's weights are here."""
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
