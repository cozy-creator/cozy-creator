# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.47.0.dev0,<0.48", "msgspec>=0.19"]
# ///
"""The `When` cell's fixture (cl-948, the web suite's fields cell): fields that apply only while another has a value, as qwen-image-2's
creativity with its prompt upsampling and SeedVR2's overlap with a batch size. `image` makes a picture of one colour and says
in `given` what its function got."""

import os
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, ImageFrame
from cozy_runtime.fields import Advanced, Image, Prompt, Range
from cozy_runtime.fields import When
from cozy_runtime.media import ImageFile

app = App()


class Box(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    prompt_upsampling: Annotated[bool, Advanced()] = True
    creativity: Annotated[Literal["precise", "balanced", "wild"], Advanced(), When("prompt_upsampling")] = "balanced"
    batch_size: Annotated[int, Range(1, 64)] | None = None
    temporal_overlap: Annotated[int, Range(0, 16), When("batch_size")] = 0


class Picture(msgspec.Struct):
    image: ImageFile
    given: str


@app.tool(label="Image", named="still")
def image(payload: Box) -> Picture:
    """A still of one colour, and what it was given."""
    given = f"creativity={payload.creativity} temporal_overlap={payload.temporal_overlap}"
    return Picture(ImageFile.from_image(ImageFrame(16, 16, os.urandom(3) * 256), format="png"), given)
