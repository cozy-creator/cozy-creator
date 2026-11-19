"""prefetch_gpuless's tools (cl-1279, cl-1286; e2e/cli/prefetch.sh): each a still once its model is built, on whatever device its
machine has. `still` (e2e/pre-req) has no time measured without a GPU; `measured` (e2e/pre-ask) has a CPU row in benchmarks.json.
`authored` (e2e/pre-req) declares a time that reads no row; `formula` (e2e/pre-ask) one that reads its CPU row."""

import os

import msgspec
from cozy_runtime.author import App, ImageFrame, Loader, Model, Seed
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Built(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Words(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


def made(payload: Words, model: Built) -> ImageResult:
    _ = model.graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)


@app.tool(label="Unmeasured", named="still", defaults={"model": "e2e/pre-req@0.1.0"})
def still(payload: Words, model: Built) -> ImageResult:
    """A still of one colour, once its model is built."""
    return made(payload, model)


@app.tool(label="Measured", named="measured", defaults={"model": "e2e/pre-ask@0.1.0"})
def measured(payload: Words, model: Built) -> ImageResult:
    """A still of one colour, once its model is built: timed without a GPU by its CPU row."""
    return made(payload, model)


@app.tool(label="Authored", named="authored", defaults={"model": "e2e/pre-req@0.1.0"}, estimate={"ms": "4000.0"})
def authored(payload: Words, model: Built) -> ImageResult:
    """A still of one colour, once its model is built: its time its author's word alone, measured on nothing."""
    return made(payload, model)


@app.tool(label="Formula", named="formula", defaults={"model": "e2e/pre-ask@0.1.0"}, estimate={"ms": "measured(machine).warm_ms * 2.0"})
def formula(payload: Words, model: Built) -> ImageResult:
    """A still of one colour, once its model is built: timed without a GPU by a formula over its CPU row."""
    return made(payload, model)
