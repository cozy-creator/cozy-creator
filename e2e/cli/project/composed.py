# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A still built from a fine-tune's transformer and a base's VAE, with a third model on demand, never read: what cl-1071's plan_fetch
quotes, a run's download being what its machine fetches (e2e/composed-* in e2e/isolated.sh). Each model is built of nothing, as
ondemand.py's: its machine fetches the components its slot reads before the run starts, and the on-demand one beside it."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Loader, Model, Seed
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Part(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Composed", named="composed", accelerator=False,
          defaults={"model": "e2e/composed-base@0.1.0", "vae": "e2e/composed-base@0.1.0", "extra": "e2e/composed-extra@0.1.0"},
          components={"model": ("transformer",), "vae": ("vae",)}, on_demand=("extra",))  # fmt: skip
def composed(payload: Words, model: Part, vae: Part, extra: Part) -> ImageResult:
    """A still of one colour, once its transformer and VAE are built; `extra` never read."""
    _ = model.graph, vae.graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
