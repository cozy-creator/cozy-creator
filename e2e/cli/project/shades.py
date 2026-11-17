# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The unseen cell's tool (cl-1105): a weightless still of one grey, near white or near black, taking `seconds` to make."""

import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Telemetry
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()
SIDE = 64


class ShadeBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    shade: Annotated[float, Range(0, 1)] = 0.5
    seconds: Annotated[float, Range(0, 600)] = 0.0


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


@app.tool(label="A shade", named="shade")
def shade(ctx: Context, payload: ShadeBox, tel: Telemetry) -> Picture:
    """A still of one grey: 0 black, 1 white."""
    with tel.stage("drawing"):
        for at in range(10):
            time.sleep(payload.seconds / 10)
            ctx.progress(at, 10)
    level = round(payload.shade * 255)
    frame = ImageFrame(SIDE * 3, SIDE * 2, bytes([level] * 3) * (SIDE * 3 * SIDE * 2))
    return Picture(ImageFile.from_image(frame, format="png"), payload.instructions.text)
