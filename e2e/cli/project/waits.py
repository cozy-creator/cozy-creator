# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A weightless still of one random colour that takes `seconds`, so a proof can do something while it runs. Its call says what it read."""

import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import Adjustments, App, ImageFrame, Outputs, Seed
from cozy_runtime.fields import AspectRatio, Image, Prompt, Range
from cozy_runtime.image import ImageRequest, ImageResult, Sizes
from cozy_runtime.media import VideoFile

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


class Kept(msgspec.Struct, kw_only=True):
    video: VideoFile


class KeptVideo(msgspec.Struct):
    video: VideoFile


@app.tool(label="Keep the video", named="kept")
def keep(payload: Kept, out: Outputs) -> KeptVideo:
    """The video it reads, as it is: a required video slot, as an upscaler has."""
    return KeptVideo(out.save_bytes(payload.video.read_bytes(), media_type="video/mp4"))
