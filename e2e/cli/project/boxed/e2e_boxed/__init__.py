"""Weightless tools for the from-source and Runtime-skew cells: each a box of declared fields."""

import os
from typing import Annotated

import msgspec
from cozy_runtime.author import App, ImageFrame
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Shaded(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    shade: Annotated[float, Range(0, 1, step=0.25)] = 0.5


class Picture(msgspec.Struct):
    image: ImageFile


def _picture() -> Picture:
    return Picture(ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"))


@app.tool()
def one_field(payload: Shaded) -> Picture:
    """A still, its shade on a range."""
    return _picture()


@app.tool()
def plain(payload: Shaded) -> Picture:
    """A still with the declared box."""
    return _picture()
