# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The tool cell's package: weightless functions as a canvas drops them. `still` has no adapter, so it's called directly, and `slow` too, saying how far it's got. `stepped`
is no tool: its stages and steps time a run as a model's do. `told` has an
adapter, whose answer says what it saw of what it reads. `need` wants a file it may not get, `clip` makes a video, `words`
makes no media, and `ask` answers text, by an adapter of kind `text`."""

import os
import time
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, File, ImageFile, ImageFrame, Node, Outputs, Telemetry, VideoFile

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


@app.entrypoint(node=Node(name="still"))
def still(payload: StillInput, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(_picture(out), payload.prompt)


class SlowInput(msgspec.Struct, kw_only=True):
    prompt: str
    seconds: float = 4.0
    seed: int | None = None


@app.entrypoint(node=Node())
def slow(payload: SlowInput, out: Outputs, tel: Telemetry) -> Picture:
    """A still that takes its time, saying how far it's got in steps."""
    step = tel.step_callback(10, stage="drawing")
    for at in range(10):
        time.sleep(payload.seconds / 10)
        step(at)
    return Picture(_picture(out), payload.prompt)


class SteppedInput(msgspec.Struct, kw_only=True):
    prompt: str = ""
    steps: int = 8
    step: float = 0.5
    first: float = 2.0
    seed: int | None = None


@app.entrypoint()
def stepped(payload: SteppedInput, out: Outputs, tel: Telemetry) -> Picture:
    """A still made as a model makes one, weightless: its prompt encoded, its steps (the first slower, as a warm-up is), decoded."""
    with tel.stage("encoding prompt"):
        time.sleep(payload.step)
    with tel.stage("generating image"):
        on_step = tel.step_callback(payload.steps, stage="generating image")
        for at in range(payload.steps):
            time.sleep(payload.first if at == 0 else payload.step)
            on_step(at)
    with tel.stage("decoding image"):
        time.sleep(payload.step)
    return Picture(_picture(out), payload.prompt)


class Beat(msgspec.Struct, kw_only=True):
    description: str = ""
    duration: Annotated[float, msgspec.Meta(extra={"min": 1, "max": 15, "step": 1, "total": True, "unit": "s"})] = 5


class Seen(msgspec.Struct, kw_only=True):
    """A prompt never crosses a mention, whatever a package asks for."""

    name: str
    description: str = ""
    instructions: str = ""
    image: ImageFile | None = None


class ToldInput(msgspec.Struct, kw_only=True):
    prompt: str
    bright: bool = True
    shade: Annotated[float | None, msgspec.Meta(extra={"min": 0, "max": 1})] = None
    aspect_ratio: Annotated[str | None, msgspec.Meta(extra={"options": ["1:1"]})] = None
    beats: Annotated[list[Beat], msgspec.Meta(min_length=1, max_length=4)]
    video: VideoFile | None = None
    secret: str = ""
    references: list[Seen] = []
    seed: int | None = None


@app.entrypoint(node=Node(name="told", hide=["secret"], label="Told"))
def told(payload: ToldInput, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(_picture(out), payload.prompt)


class NeedInput(msgspec.Struct, kw_only=True):
    video: VideoFile


class Clip(msgspec.Struct):
    video: VideoFile


@app.entrypoint(node=Node(ui="ui/clip.js"))
def need(payload: NeedInput, out: Outputs) -> Clip:
    """Keeps a video as it is."""
    with payload.video.stream() as events:
        return Clip(out.save_video_stream(iter(events)).video)


class ClipInput(msgspec.Struct, kw_only=True):
    resolution: Literal["720p", "1080p"] = "720p"


@app.entrypoint(node=Node())
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


class CaptionInput(msgspec.Struct, kw_only=True):
    images: list[ImageFile] = []
    videos: list[VideoFile] = []


class Asked(msgspec.Struct):
    texts: list[str]


@app.entrypoint()
def caption(payload: CaptionInput) -> Asked:
    """Answers about what it's shown, in the words it chooses."""
    return Asked([f"THE-PACKAGE-ASKS {len(payload.images)} pictures, {len(payload.videos)} videos."])
