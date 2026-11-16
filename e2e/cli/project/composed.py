# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A still that reads a fine-tune's transformer and a base's VAE as raw weights, and has a model it builds, and so fetches, only on
demand, with `read_extra` (as ondemand.py's): what cl-1071's plan_fetch quotes, a run's download being what it fetches (e2e/composed-*
in e2e/isolated.sh)."""

import os

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Loader, Model, Seed, Weights
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None
    #: Reads `extra`: its first read builds it, its weights fetched inside the run.
    read_extra: bool = False


@app.tool(label="Composed", named="composed", accelerator=False,
          defaults={"model": "e2e/composed-base@0.1.0", "vae": "e2e/composed-base@0.1.0", "extra": "e2e/composed-extra@0.1.0"},
          components={"model": ("transformer",), "vae": ("vae",)}, on_demand=("extra",))  # fmt: skip
async def composed(ctx: Context, payload: Words, model: Weights, vae: Weights, extra: Extra) -> ImageResult:
    """A still of one colour, made once its transformer and VAE are here; `extra` built only with `read_extra`."""
    if payload.read_extra:
        _ = extra.graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
