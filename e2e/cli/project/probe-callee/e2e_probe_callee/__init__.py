"""The tools cells' second package. `shade` is weightless, on the CPU, and memoized: a composed tool in `e2e-probe` calls it and
branches on `dark`, and `hold` keeps it going that long. `enlarge` runs a model sharded 2 or 4 wide, so the caller's `calls` show its
GPUs; nothing calls it unless asked."""

import time
from typing import Annotated

import msgspec
from cozy_runtime.accel import sequence_parallel
from cozy_runtime.author import App, ImageFrame, Loader, Model, Outputs, uses_components
from cozy_runtime.fields import Range
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class ShadeInput(msgspec.Struct, kw_only=True):
    image: ImageFile
    amount: Annotated[float, Range(0.0, 1.0, step=0.25)] = 0.5
    hold: Annotated[float, Range(0, 60, unit="s")] = 0


class Shaded(msgspec.Struct):
    image: ImageFile
    #: Whether the shaded picture came out dark: the caller branches on it.
    dark: bool


@app.entrypoint(accelerator=False, memoize=True)
def shade(payload: ShadeInput, out: Outputs) -> Shaded:
    """A flat grey still, as dark as `amount` asks."""
    time.sleep(payload.hold)
    level = round(255 * (1 - payload.amount))
    return Shaded(out.save_image(ImageFrame(SIDE, SIDE, bytes([level]) * 3 * SIDE**2), format="png"), level < 128)


@sequence_parallel(degrees=(2, 4))
class Upscaler(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(object)

    @uses_components("transformer")
    def forward[T](self, pixels: T) -> T:
        return pixels


class EnlargeInput(msgspec.Struct, kw_only=True):
    image: ImageFile


class Enlarged(msgspec.Struct):
    image: ImageFile


@app.entrypoint(defaults={"model": "e2e/upscaler@0.1.0"})
def enlarge(payload: EnlargeInput, model: Upscaler, out: Outputs) -> Enlarged:
    """The still, twice the size."""
    return Enlarged(payload.image)
