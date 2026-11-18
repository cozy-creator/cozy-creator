# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19"]
# ///
"""cl-1242's tool: a still that holds for `seconds` in a stage saying its prompt's words, as a package's stage may say what it was
asked ("upsampling <prompt>"), so a cell reads where a project's words reach while its run goes."""
import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Telemetry
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()


class SaidBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seconds: Annotated[float, Range(0, 600)] = 60.0


class Picture(msgspec.Struct):
    image: ImageFile


@app.tool(label="Said")
def said(ctx: Context, payload: SaidBox, tel: Telemetry) -> Picture:
    """A still of one colour, after a while thinking of its words, a step every two seconds."""
    steps = max(1, int(payload.seconds / 2))
    with tel.stage(f"thinking of {payload.instructions.text.strip()}"):
        for at in range(steps):
            time.sleep(payload.seconds / steps)
            ctx.progress(at + 1, steps)
    return Picture(ImageFile.from_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png"))
