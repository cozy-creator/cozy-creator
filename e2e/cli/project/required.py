# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45.0", "msgspec>=0.19"]
# ///
"""The required-choice cell's fixture (cl-1066, Paul 11-16 on minimax-h3/edit's `edit`: "that 'edit needed' is so dumb"): a choice
with no default and words for its options. A render of a box that hasn't chosen is refused before any machine, in the box's words."""

import os
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, ImageFrame
from cozy_runtime.media import ImageFile

app = App()

OPTIONS = {"character": "Character swap", "head": "Head swap", "style": {"words": "Style transfer", "description": "Paints it in another look"}}


class Box(msgspec.Struct, kw_only=True):
    edit: Annotated[Literal["character", "head", "style"], msgspec.Meta(extra={"label": "Edit", "options": OPTIONS})]


class Picture(msgspec.Struct):
    image: ImageFile


@app.tool(label="Pick", named="pick")
def pick(payload: Box) -> Picture:
    """A still of one colour, once an edit is chosen."""
    return Picture(ImageFile.from_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png"))
