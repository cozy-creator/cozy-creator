"""The web-package-ui cell's package: weightless functions, each naming a component in `ui/`. `one_field` draws one field of its box and
`whole_face` all of it; `throws` throws, `later` speaks an API this page doesn't, `gone` names a file it doesn't ship, and `plain` ships none."""

import os
from typing import Annotated

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Shaded(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    shade: Annotated[float, Range(0, 1, step=0.25)] = 0.5


class Picture(msgspec.Struct):
    image: ImageFile


def _picture(out: Outputs) -> Picture:
    return Picture(out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"))


@app.tool(ui="ui/shade.js")
def one_field(payload: Shaded, out: Outputs) -> Picture:
    """A still, its shade chosen on a row of swatches."""
    return _picture(out)


@app.tool(ui="ui/whole.js")
def whole_face(payload: Shaded, out: Outputs) -> Picture:
    """A still whose whole box is its own."""
    return _picture(out)


@app.tool(ui="ui/throws.js")
def throws(payload: Shaded, out: Outputs) -> Picture:
    """A still whose component throws as it draws."""
    return _picture(out)


@app.tool(ui="ui/later.js")
def later(payload: Shaded, out: Outputs) -> Picture:
    """A still whose component speaks an API from later."""
    return _picture(out)


@app.tool(ui="ui/gone.js")
def gone(payload: Shaded, out: Outputs) -> Picture:
    """A still whose component isn't in its package."""
    return _picture(out)


@app.tool()
def plain(payload: Shaded, out: Outputs) -> Picture:
    """A still with the declared box."""
    return _picture(out)
