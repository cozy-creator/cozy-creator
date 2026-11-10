# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43", "msgspec>=0.19"]
# ///
"""A weightless still with a Turbo toggle, as a model's tool has one: one box, its Turbo a field."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import ImageFile

app = App()


class Quick(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    turbo: bool = False


class Picture(msgspec.Struct):
    image: ImageFile


@app.tool(label="Quick still", named="still")
def still(payload: Quick, out: Outputs) -> Picture:
    """A still of one colour; Turbo changes nothing but the field."""
    return Picture(out.save_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png"))
