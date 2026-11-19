"""E2E Words: an image tool whose box reads words only, no picture, as a third party's may (se-175). The web suite's isolated Hub
holds it beside the first-party packages, which all read pictures now: a picture its words start from is struck through, and
its `<` offers none. Its picture is one colour; it needs no model."""

import os

import msgspec

from cozy_runtime.author import App, ImageFrame, Seed
from cozy_runtime.media import ImageFile

app = App()
SIDE = 64


class Box(msgspec.Struct, kw_only=True):
    instructions: str = ""
    seed: Seed | None = None


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


@app.tool(label="Image", named="image")
def image(payload: Box) -> Picture:
    """A picture of one colour, from words alone."""
    return Picture(ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"), payload.instructions)
