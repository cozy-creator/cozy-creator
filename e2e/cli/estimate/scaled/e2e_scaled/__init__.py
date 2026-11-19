"""estimate_scaled's tool (cl-1286, e2e/cli/estimate.sh): no model, so it renders on any computer. Its time grows as its seconds^0.5,
as its CPU rows (benchmarks.json) measure it: a past render of it scales to another size by that exponent, never in proportion."""

import os
import time

import msgspec
from cozy_runtime.author import App, Context, ImageFrame
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Ask(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seconds: float = 4.0


@app.tool(label="Scaled", named="scaled", estimate={"size": {"seconds": "request.seconds"}})
def scaled(ctx: Context, payload: Ask) -> ImageResult:
    """A still of one colour, after seconds^0.5 s."""
    time.sleep(payload.seconds**0.5)
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, 0, payload.instructions.text)
