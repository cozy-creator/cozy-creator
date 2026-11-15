"""CPU author handlers exercised only through the public Cozy CLI."""

from collections.abc import Iterator
from fractions import Fraction
from typing import Annotated, Literal, cast

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs, call
from cozy_runtime.fields import FileBound
from cozy_runtime.media import (
    DecodedMediaEvent,
    DecodedMediaHeader,
    DecodedVideoFormat,
    DecodedVideoFrame,
    ImageFile,
    VideoFile,
)
from PIL import Image

app = App()


class Request(msgspec.Struct):
    width: int = 4
    height: int = 3
    format: Literal["png", "webp"] | None = None


class Result(msgspec.Struct):
    image: ImageFile
    width: int
    height: int


@app.entrypoint(accelerator=False)
def generate(payload: Request) -> Result:
    image = Image.new("RGBA", (payload.width, payload.height), (15, 63, 127, 95))
    value = ImageFile.from_image(image, format=payload.format).with_metadata(
        {"case": "returned", "width": payload.width, "height": payload.height}
    )
    image.paste((0, 0, 0, 0), (0, 0, payload.width, payload.height))
    return Result(value, payload.width, payload.height)


@app.entrypoint(accelerator=False)
def intermediate(payload: Request, out: Outputs) -> Result:
    earlier = out.save_image(ImageFrame(2, 2, bytes([255, 0, 0]) * 4), format="png")
    out.publish("image", earlier)
    return generate(payload)


class Nested(msgspec.Struct):
    images: list[ImageFile]


class NestedResult(msgspec.Struct):
    nested: Nested


@app.entrypoint(accelerator=False)
def nested(payload: Request) -> NestedResult:
    return NestedResult(
        Nested([ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))])
    )


class DuplicateResult(msgspec.Struct):
    first: ImageFile
    second: ImageFile


@app.entrypoint(accelerator=False)
def duplicate(payload: Request) -> DuplicateResult:
    value = ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))
    return DuplicateResult(value, value)


class BoundedResult(msgspec.Struct):
    image: Annotated[ImageFile, FileBound(max_decoded_bytes=4)]


@app.entrypoint(accelerator=False)
def decoded_bound(payload: Request) -> BoundedResult:
    return BoundedResult(ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4)))


class VideoResult(msgspec.Struct):
    video: VideoFile
    frames: int


@app.entrypoint(accelerator=False)
def streaming(payload: Request, out: Outputs) -> VideoResult:
    def events() -> Iterator[DecodedMediaEvent]:
        yield DecodedMediaHeader(
            DecodedVideoFormat(
                16, 16, Fraction(1, 24), Fraction(1), Fraction(24), 1, 1, 1, 2
            ),
            None,
        )
        for at in range(3):
            yield DecodedVideoFrame(
                16,
                16,
                bytes([15 + at, 63, 127]) * 256,
                at,
                1,
                Fraction(1, 24),
                Fraction(1),
                1,
                1,
                1,
                2,
            )

    saved = out.save_video_stream(events())
    out.publish("video", saved.video)
    return VideoResult(saved.video, saved.frame_count)


@app.entrypoint(accelerator=False)
async def child_forward(payload: Request) -> Result:
    return await call(generate, payload=payload)


@app.entrypoint(accelerator=False)
async def child_nested(payload: Request) -> NestedResult:
    child = await call(generate, payload=payload)
    return NestedResult(Nested([child.image]))


@app.entrypoint(accelerator=False)
async def child_failure(payload: Request) -> Result:
    return await call(generate, payload=Request(width=16384, height=1, format="webp"))


class EncodedBoundResult(msgspec.Struct):
    image: Annotated[ImageFile, FileBound(max_bytes=1)]


@app.entrypoint(accelerator=False)
def encoded_bound(payload: Request) -> EncodedBoundResult:
    return EncodedBoundResult(
        ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))
    )


@app.entrypoint(accelerator=False)
def legacy_encoded_bound(payload: Request, out: Outputs) -> EncodedBoundResult:
    return EncodedBoundResult(
        out.save_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4), format="webp")
    )


@app.entrypoint(accelerator=False)
def invalid_shape(payload: Request) -> Result:
    return Result(ImageFile.from_image(ImageFrame(4, 3, b"too short")), 4, 3)


@app.entrypoint(accelerator=False)
def invalid_kind(payload: Request) -> Result:
    return Result(ImageFile.from_image(cast(ImageFrame, b"not pixels")), 4, 3)


@app.entrypoint(accelerator=False)
def foreign(payload: Request) -> Result:
    return Result(ImageFile("not-this-attempt"), 4, 3)


@app.entrypoint(accelerator=False)
def empty_format(payload: Request) -> Result:
    image = ImageFile.from_image(
        ImageFrame(2, 2, bytes([7, 11, 19]) * 4),
        format=cast(Literal["png", "webp"], ""),
    )
    return Result(image, 2, 2)


class AliasedResult(msgspec.Struct):
    first: Nested
    second: Nested


@app.entrypoint(accelerator=False)
def aliased_return(payload: Request) -> AliasedResult:
    shared = Nested([ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))])
    return AliasedResult(shared, shared)


@app.entrypoint(accelerator=False)
async def aliased_child(payload: Request) -> AliasedResult:
    child = await call(generate, payload=payload)
    shared = Nested([child.image])
    return AliasedResult(shared, shared)
