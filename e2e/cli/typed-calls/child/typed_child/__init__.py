"""A real CPU child with its own compute dependency, media and raw local model reads."""

from array import array
import sys
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Weights, call
from cozy_runtime.fields import FileBound
from cozy_runtime.media import Files, ImageFile

from .leaf_calls import PrimeRequest, SquareRequest, judge, square

app = App()


class Request(msgspec.Struct, kw_only=True):
    image: ImageFile
    number: int = 17
    fail: bool = False


class Result(msgspec.Struct, kw_only=True):
    image: Annotated[ImageFile, FileBound(max_bytes=1 << 20)]
    prime: bool
    python: str
    descendant_python: str
    fingerprint: str
    squared: int


@app.entrypoint(accelerator=False)
async def shade(payload: Request) -> Result:
    decision = await call(judge, PrimeRequest(number=payload.number, fail=payload.fail))
    computed = await call(square, SquareRequest(number=payload.number))
    image = payload.image.read().convert("RGBA")
    image.putpixel((0, 0), (7, 11, 19, 95))
    return Result(
        image=ImageFile.from_image(image, format="png"),
        prime=decision.prime,
        python=sys.executable,
        descendant_python=decision.python,
        fingerprint=decision.fingerprint,
        squared=computed.value,
    )


class WeightRequest(msgspec.Struct, kw_only=True):
    pass


class WeightResult(msgspec.Struct, kw_only=True):
    checkpoint: str
    value: float
    python: str


@app.entrypoint(accelerator=False)
def weighted(payload: WeightRequest, weights: Weights) -> WeightResult:
    component = weights.components()[0]
    key = weights.keys(component)[0]
    values = array("f", [0.0])
    weights.read_values_into(component, key, 0, values)
    return WeightResult(
        checkpoint=weights.checkpoint_ref, value=values[0], python=sys.executable
    )


@app.entrypoint(accelerator=False, private=True)
def private(payload: WeightRequest) -> WeightResult:
    return WeightResult(checkpoint="private", value=0, python=sys.executable)


class AssetRequest(msgspec.Struct, kw_only=True):
    number: int = 17
    fail: bool = False


@app.entrypoint(accelerator=False)
async def shade_assets(payload: AssetRequest, pictures: Files[ImageFile]) -> Result:
    # This ordinary local call stays in this attempt; shade itself makes the managed leaf call.
    return await shade(
        Request(image=pictures[0], number=payload.number, fail=payload.fail)
    )
