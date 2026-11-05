# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""A weightless still of one random colour that takes `seconds`, so a proof can do something while it runs. Its call says what it read."""

import os
import time

import msgspec
from cozy_runtime.author import Adjustments, App, File, ImageFrame, ImageRequest, ImageResult, Outputs, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


class Seen(msgspec.Struct, kw_only=True):
    name: str
    image: File | None = None


class Waiting(ImageRequest):
    prompt: str = ""
    pictures: list[Seen] = msgspec.field(default_factory=list)
    seconds: float = 0


@app.entrypoint()
def wait(payload: Waiting, adj: Adjustments, out: Outputs) -> ImageResult:
    time.sleep(payload.seconds)
    width, height = payload.size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)
