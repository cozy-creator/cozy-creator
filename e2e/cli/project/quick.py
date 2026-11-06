# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.41", "msgspec>=0.19"]
# ///
"""A weightless still and its Turbo: two functions of one label, so one box with a toggle that flips between them."""

import os

import msgspec
from cozy_runtime.author import App, ImageFile, ImageFrame, Node, Outputs

app = App()


class Ask(msgspec.Struct, kw_only=True):
    prompt: str


class Picture(msgspec.Struct):
    image: ImageFile


def _picture(out: Outputs) -> Picture:
    return Picture(out.save_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png"))


@app.entrypoint(node=Node(label="Quick still"))
def still(payload: Ask, out: Outputs) -> Picture:
    """A still of one colour."""
    return _picture(out)


@app.entrypoint(node=Node(label="Quick still", variant="Turbo"))
def still_turbo(payload: Ask, out: Outputs) -> Picture:
    """The same, sooner."""
    return _picture(out)
