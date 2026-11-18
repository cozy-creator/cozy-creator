# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""steady-face's tool (cl-1197): three stages that each come and go in a blink, `blink` seconds, as phases with nothing to do, then the
still's own work, `seconds`. A face that shows every phase flashes them; one that waits for a phase to last shows none. Its seed makes
each render a new one: a box rendered again as it was is answered from its take, and no run comes."""

import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, ImageFrame, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16
BLINKS = ("blinking once", "blinking twice", "blinking a third time")


class Blink(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    blink: Annotated[float, Range(0, 2, unit="s")] = 0.2
    seconds: Annotated[float, Range(0, 30, unit="s")] = 3


@app.tool(label="Blink, then draw", named="blink")
def blink(payload: Blink, tel: Telemetry) -> ImageResult:
    for stage in BLINKS:
        with tel.stage(stage):
            time.sleep(payload.blink)
    with tel.stage("drawing"):
        time.sleep(payload.seconds)
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
