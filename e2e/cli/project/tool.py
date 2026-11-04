# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The tool cell's package: weightless functions as a canvas drops them. `still` has no adapter, so it's called directly. `told` has an
adapter told its name, whose answer says what it saw of what it reads. `need` wants a file it may not get, `clip` makes a video, `words`
makes no media, and `ask` answers text, by an adapter of kind `text`."""

import json
import os
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, File, ImageFile, ImageFrame, Invocation, Operation, Outputs, VideoFile

app = App()
SIDE = 16


class Named(msgspec.Struct, kw_only=True):
    name: str
    image: ImageFile


class StillInput(msgspec.Struct, kw_only=True):
    prompt: str
    pictures: Annotated[list[Named], msgspec.Meta(max_length=1)] = []
    count: Annotated[int, msgspec.Meta(ge=1, le=4)] = 1
    aspect_ratio: Literal["1:1", "16:9"] = "1:1"
    style: Literal["flat", "noise"] = "flat"
    soft: bool = False
    seed: int | None = None


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


def _picture(out: Outputs) -> ImageFile:
    return out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")


@app.entrypoint()
def still(payload: StillInput, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(_picture(out), payload.prompt)


class Beat(msgspec.Struct, kw_only=True):
    description: str = ""
    duration: Annotated[float | None, msgspec.Meta(extra={"max_duration": 15})] = None


class ToldFields(msgspec.Struct, kw_only=True):
    instructions: str = ""
    shade: Annotated[float | None, msgspec.Meta(extra={"min": 0, "max": 1})] = None
    aspect_ratio: Annotated[str | None, msgspec.Meta(extra={"options": ["1:1"]})] = None
    beats: list[Beat] = []
    video: Annotated[str, msgspec.Meta(extra={"accept": "video"})] = ""
    secret: Annotated[str, msgspec.Meta(extra={"hidden": True})] = ""


class ToldOperation(Operation, kw_only=True):
    fields: ToldFields


class ToldInput(msgspec.Struct, kw_only=True):
    prompt: str
    seed: int | None = None


@app.entrypoint()
def told(payload: ToldInput, out: Outputs) -> Picture:
    """A still of one colour, by its adapter."""
    return Picture(_picture(out), payload.prompt)


def _heard(op: Operation) -> list[Invocation]:
    heard = {"function": op.function, "fields": msgspec.to_builtins(op.fields), "options": op.options, "reads": [msgspec.to_builtins(read.fields) for read in op.reads]}
    return [Invocation(function=op.function or "told", input={"prompt": json.dumps(heard)})]


@app.project_adapter("image")
def adapt_alone(op: Operation) -> list[Invocation]:
    return _heard(op)


@app.project_adapter("image", functions=["told"])
def adapt_told(op: ToldOperation) -> list[Invocation]:
    return _heard(op)


class NeedInput(msgspec.Struct, kw_only=True):
    video: VideoFile


class Clip(msgspec.Struct):
    video: VideoFile


@app.entrypoint(ui="ui/clip.js")
def need(payload: NeedInput, out: Outputs) -> Clip:
    """Keeps a video as it is."""
    with payload.video.stream() as events:
        return Clip(out.save_video_stream(iter(events)).video)


class ClipInput(msgspec.Struct, kw_only=True):
    resolution: Literal["720p", "1080p"] = "720p"


@app.entrypoint()
def clip(payload: ClipInput, out: Outputs) -> Clip:
    """Never run: a function whose output is a video."""
    raise NotImplementedError(payload.resolution)


class WordsInput(msgspec.Struct, kw_only=True):
    prompt: str
    file: File | None = None


class Said(msgspec.Struct):
    text: str


@app.entrypoint()
def words(payload: WordsInput) -> Said:
    """Makes no media, so it's no tool."""
    return Said(payload.prompt)


class AskInput(msgspec.Struct, kw_only=True):
    prompt: str
    images: list[ImageFile] = []
    videos: list[VideoFile] = []


class Asked(msgspec.Struct):
    texts: list[str]


@app.entrypoint()
def ask(payload: AskInput) -> Asked:
    """Answers about what it's shown."""
    return Asked([f"{payload.prompt} {len(payload.images)} pictures, {len(payload.videos)} videos."])


@app.project_adapter("text")
def adapt_text(op: Operation) -> list[Invocation]:
    read = op.reads[0]
    return [Invocation(function="ask", input={"prompt": "THE-PACKAGE-ASKS", "images": [file.sha256 for file in read.files_of("image")], "videos": [file.sha256 for file in read.files_of("video")]})]
