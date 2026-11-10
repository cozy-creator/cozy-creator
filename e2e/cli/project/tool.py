# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The tool cells' package: weightless tools as a canvas drops them. `still` makes a picture of one colour, `slow` too, saying how far
it's got in steps, and `told` a picture from a prompt and its beats. `need` wants a video it may not get. `words` makes no media, so it's no
tool, and `caption` answers what it's shown, as Calcifer's Caption asks a package's `caption`. `stepped` is no tool either: its stages and
steps time a run as a model's do."""

import os
import time
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, ImageFrame, Outputs, Seed, Telemetry
from cozy_runtime.fields import AspectRatio, Audio, Count, Image, Prompt, Range, Ratios, Total, Video
from cozy_runtime.media import File, ImageFile, VideoFile

app = App()
SIDE = 16


class StillBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    count: Annotated[int, Range(1, 4)] = 1
    aspect_ratio: Annotated[AspectRatio, Ratios("1:1", "16:9")] | None = None
    style: Literal["flat", "noise"] = "flat"
    soft: bool = False
    seed: Seed | None = None


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


def _picture(out: Outputs) -> ImageFile:
    return out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")


@app.tool(label="A still", named="still")
def still(payload: StillBox, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(_picture(out), payload.instructions.text)


class SlowBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seconds: Annotated[float, Range(0, 60)] = 4.0
    seed: Seed | None = None


@app.tool(label="A slow still")
def slow(payload: SlowBox, out: Outputs, tel: Telemetry) -> Picture:
    """A still that takes its time, saying how far it's got in steps."""
    step = tel.step_callback(10, stage="drawing")
    for at in range(10):
        time.sleep(payload.seconds / 10)
        step(at)
    return Picture(_picture(out), payload.instructions.text)


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
    description: Prompt[Image | Video | Audio] = Prompt()
    duration: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 5


class ToldBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image | Video | Audio] = Prompt()
    bright: bool = True
    shade: Annotated[float, Range(0, 1)] | None = None
    aspect_ratio: Annotated[AspectRatio, Ratios("1:1")] | None = None
    beats: Annotated[list[Beat], Count(min=1, max=4)]
    video: VideoFile | None = None
    seed: Seed | None = None


@app.tool(label="Told", named="told")
def told(payload: ToldBox, out: Outputs) -> Picture:
    """A still of one colour, from a prompt and its beats."""
    return Picture(_picture(out), payload.instructions.text)


class NeedBox(msgspec.Struct, kw_only=True):
    video: VideoFile


class Clip(msgspec.Struct):
    video: VideoFile


@app.tool(label="Keep a video")
def need(payload: NeedBox, out: Outputs) -> Clip:
    """Keeps a video as it is."""
    with payload.video.stream() as events:
        return Clip(out.save_video_stream(iter(events)).video)


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
    image: Image | None = None
    video: Video | None = None


class Asked(msgspec.Struct):
    texts: list[str]


@app.entrypoint()
def caption(payload: CaptionInput) -> Asked:
    """Answers about what it's shown, in the words it chooses."""
    return Asked([f"THE-PACKAGE-ASKS {int(payload.image is not None)} pictures, {int(payload.video is not None)} videos."])
