"""disk_room's published tool (cl-1175, e2e/cli/disk.sh): a still built from e2e/disk-big, 2 GiB of random weights (e2e/isolated.sh's
`disk`), more than a 2.5 GiB disk with its 1 GiB reserve has room for. Its machine fetches them before the run starts."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Loader, Model, Seed
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Weights(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Roomless", named="still", accelerator=False, defaults={"weights": "e2e/disk-big@0.1.0"})
def still(payload: Words, weights: Weights) -> ImageResult:
    """A still of one colour, once its weights are built."""
    _ = weights.graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)
