# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43", "msgspec>=0.19"]
# ///
"""The project cell's picture tool: a weightless still of one random colour, so each version differs. Its call says what it read."""

import os
from typing import Annotated

import msgspec
from cozy_runtime.author import Adjustments, App, ImageFrame, Outputs, Seed
from cozy_runtime.fields import AspectRatio, Image, Prompt, Range
from cozy_runtime.image import ImageRequest, ImageResult, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Drawing(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    aspect_ratio: AspectRatio | None = None
    megapixels: Annotated[float, Range(0.25, 4, step=0.25, unit="MP")] | None = None
    seed: Seed | None = None


@app.tool(label="Draw", named="drawing")
def draw(payload: Drawing, adj: Adjustments, out: Outputs) -> ImageResult:
    asked = ImageRequest(payload.instructions.text, payload.aspect_ratio, payload.megapixels, payload.seed)
    width, height = asked.size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.instructions.text)
