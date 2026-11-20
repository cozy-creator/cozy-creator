"""prefetch_yields' LoRA tool (cl-1281, e2e/cli/prefetch.sh): Orbit's shape, a `lora` slot that is a Weights with a default
(e2e/pre-lora-a, 128 MiB), which a box replaces by `models.lora`. Its machine fetches the slot's LoRA before the run starts."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Seed, Weights
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Prefetched LoRA", named="styled", accelerator=False, defaults={"lora": "e2e/pre-lora-a@0.1.0"})
def styled(payload: Words, lora: Weights) -> ImageResult:
    """A still of one colour, once its LoRA is there."""
    _ = lora
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
