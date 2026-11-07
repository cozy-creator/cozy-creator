# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43", "msgspec>=0.19"]
# ///
"""A weightless still of one random colour that takes `seconds`, so a proof can do something while it runs. Its call says what it read."""

import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import Adjustments, App, AspectRatio, Image, ImageFrame, ImageRequest, ImageResult, Outputs, Prompt, Range, Seed, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Waiting(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    aspect_ratio: AspectRatio | None = None
    megapixels: Annotated[float, Range(0.25, 4, step=0.25, unit="MP")] | None = None
    seconds: Annotated[float, Range(0, 600, unit="s")] = 0
    seed: Seed | None = None


@app.tool(label="Wait, then draw", named="waiting")
def wait(payload: Waiting, adj: Adjustments, out: Outputs) -> ImageResult:
    time.sleep(payload.seconds)
    asked = ImageRequest(payload.instructions.text, payload.aspect_ratio, payload.megapixels, payload.seed)
    width, height = asked.size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
