"""CPU author handlers exercised only through the public Cozy CLI."""

import hashlib
from collections.abc import Iterator, Sequence
from fractions import Fraction
from typing import Annotated, Literal, cast

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs, call
from cozy_runtime.fields import FileBound
from cozy_runtime.media import (
    DecodedAudioChunk,
    DecodedAudioFormat,
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
    earlier = ImageFile.from_image(
        ImageFrame(2, 2, bytes([255, 0, 0]) * 4), format="png"
    )
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


def _clip(red: int) -> Iterator[DecodedMediaEvent]:
    """Three silent 16x16 frames of one colour, on a zero-based 24 fps clock."""
    yield DecodedMediaHeader(
        DecodedVideoFormat(16, 16, Fraction(1, 24), Fraction(1), Fraction(24), 1, 1, 1, 2),
        None,
    )
    for at in range(3):
        yield DecodedVideoFrame(
            16, 16, bytes([red, 63, 127]) * 256, at, 1, Fraction(1, 24), Fraction(1), 1, 1, 1, 2
        )


@app.entrypoint(accelerator=False)
def concat_saved(payload: Request, out: Outputs) -> VideoResult:
    """Two videos it saved itself, joined by `save_video_concat`; only the join is returned
    (H3's pickup without latents: minimax-h3 join_kept, packages#953)."""
    sound = DecodedAudioFormat(
        channels=1, sample_rate=48000, channel_layout="mono", channel_names=("FC",),
        time_base=Fraction(1, 48000),
    )  # fmt: skip

    def soundtrack(counts: Sequence[int]) -> Iterator[DecodedMediaEvent]:
        samples = sum(counts) * 48000 // 24
        yield DecodedMediaHeader(None, sound)
        yield DecodedAudioChunk(
            channels=1, sample_count=samples, sample_rate=48000, channel_layout="mono",
            channel_names=("FC",), pcm_f32le=(bytes(4 * samples),), pts=0,
            time_base=Fraction(1, 48000),
        )  # fmt: skip

    first = out.save_video_stream(_clip(15)).video
    second = out.save_video_stream(_clip(200)).video
    joined = out.save_video_concat([first, second], soundtrack)
    return VideoResult(joined.video, joined.frame_count)


@app.entrypoint(accelerator=False)
async def child_forward(payload: Request) -> Result:
    return await call(generate, payload)


@app.entrypoint(accelerator=False)
async def child_nested(payload: Request) -> NestedResult:
    child = await call(generate, payload)
    return NestedResult(Nested([child.image]))


@app.entrypoint(accelerator=False)
async def child_failure(payload: Request) -> Result:
    return await call(generate, Request(width=16384, height=1, format="webp"))


class EncodedBoundResult(msgspec.Struct):
    image: Annotated[ImageFile, FileBound(max_bytes=1)]


@app.entrypoint(accelerator=False)
def encoded_bound(payload: Request) -> EncodedBoundResult:
    return EncodedBoundResult(
        ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))
    )


@app.entrypoint(accelerator=False)
def encoded_bound_copy(payload: Request) -> EncodedBoundResult:
    image = ImageFile.from_image(
        ImageFrame(2, 2, bytes([7, 11, 19]) * 4), format="webp"
    )
    return EncodedBoundResult(image.with_label("bounded copy"))


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
    child = await call(generate, payload)
    shared = Nested([child.image])
    return AliasedResult(shared, shared)


class ImageRequest(msgspec.Struct):
    image: ImageFile


class ImageFacts(msgspec.Struct):
    digest: str
    width: int
    height: int
    pixel: tuple[int, int, int, int]


@app.entrypoint(private=True, accelerator=False)
def inspect_image(payload: ImageRequest) -> ImageFacts:
    data = payload.image.read_bytes()
    decoded = payload.image.read(mode="RGBA")
    return ImageFacts(
        "sha256:" + hashlib.sha256(data).hexdigest(),
        decoded.width,
        decoded.height,
        cast(tuple[int, int, int, int], decoded.getpixel((0, 0))),
    )


class ChildInputResult(Result):
    original_digest: str
    seen: ImageFacts
    probed: tuple[int, int]


async def inspect_pending(payload: Request, *, labeled: bool) -> ChildInputResult:
    made = generate(payload)
    geometry = made.image.probe()
    offered = made.image.with_label("Picture 1") if labeled else made.image
    seen = await call(inspect_image, ImageRequest(offered))
    # The original must resolve the file adopted through a labeled copy as well.
    digest = "sha256:" + hashlib.sha256(made.image.read_bytes()).hexdigest()
    return ChildInputResult(
        made.image,
        made.width,
        made.height,
        digest,
        seen,
        (geometry.width, geometry.height),
    )


@app.entrypoint(accelerator=False)
async def child_input(payload: Request) -> ChildInputResult:
    return await inspect_pending(payload, labeled=False)


@app.entrypoint(accelerator=False)
async def child_input_label(payload: Request) -> ChildInputResult:
    return await inspect_pending(payload, labeled=True)


class ProbedResult(Result):
    probed: tuple[int, int]


@app.entrypoint(accelerator=False)
def pending_probe(payload: Request) -> ProbedResult:
    made = generate(payload)
    geometry = made.image.probe()
    return ProbedResult(
        made.image, made.width, made.height, (geometry.width, geometry.height)
    )


class PublishedResult(ProbedResult):
    original_digest: str


@app.entrypoint(accelerator=False)
def publish_copy(payload: Request, out: Outputs) -> PublishedResult:
    made = generate(payload)
    geometry = made.image.probe()
    out.publish("image", made.image.with_label("Picture 1"))
    digest = "sha256:" + hashlib.sha256(made.image.read_bytes()).hexdigest()
    return PublishedResult(
        made.image, made.width, made.height, (geometry.width, geometry.height), digest
    )


class ApiResult(Result):
    save_image_absent: bool


@app.entrypoint(accelerator=False)
def api_removed(payload: Request) -> ApiResult:
    made = generate(payload)
    return ApiResult(
        made.image, made.width, made.height, not hasattr(Outputs, "save_image")
    )


@app.entrypoint(accelerator=False)
def duplicate_label(payload: Request) -> DuplicateResult:
    image = ImageFile.from_image(ImageFrame(2, 2, bytes([7, 11, 19]) * 4))
    return DuplicateResult(image, image.with_label("same value"))
