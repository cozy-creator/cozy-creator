"""The prefetch cells' published tool (cl-1104, e2e/cli/prefetch.sh): a still built from a transformer and a VAE, as composed.py's.
Its `model` is e2e/pre-a's transformer unless its box names e2e/pre-b's; either way its VAE is e2e/pre-a's, which e2e/pre-b shares
(e2e/isolated.sh's `pre`), as two fine-tunes of one base share its text encoder and VAE. Each model is built of nothing: its machine
fetches the components its slots read before the run starts."""

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


@app.tool(label="Prefetched", named="still", accelerator=False,
          defaults={"model": "e2e/pre-a@0.1.0", "vae": "e2e/pre-a@0.1.0"},
          components={"model": ("transformer",), "vae": ("vae",)})  # fmt: skip
def still(payload: Words, model: Part, vae: Part) -> ImageResult:
    """A still of one colour, once its transformer and VAE are built."""
    _ = model.graph, vae.graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
