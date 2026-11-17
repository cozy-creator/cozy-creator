"""prefetch_disk's slow install (cl-1104, e2e/cli/prefetch.sh): a still of one colour, weightless. Its environment takes a 96 MiB
wheel from a server on this runner (wheel.py), which sends it slowly while the cell runs: a guess's build of it holds the install
line long enough for a run's install to come for the line."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Seed
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Slow install", named="still", accelerator=False)
def still(payload: Words) -> ImageResult:
    """A still of one colour."""
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
