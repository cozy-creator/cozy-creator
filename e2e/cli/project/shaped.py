# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43", "msgspec>=0.19"]
# ///
"""The node cell's package: weightless tools in a video model's own shape, so what a project gives a function can be read from its run
with no model. `shot` takes a summary and beats, a first and last frame, a plate and a pickup. `up` is an upscale whose video slot reads
the latents of a take this package made. Each makes a short grey clip of its own. `picture` takes pictures by its prompt."""

from typing import Annotated, Literal

import msgspec
import numpy as np
from cozy_runtime.author import App, ImageFrame, Outputs, Seed
from cozy_runtime.fields import (
    Advanced,
    AspectRatio,
    Count,
    Image,
    Pickup,
    Placeholder,
    Prompt,
    Range,
    Ratios,
    Snap,
    Spans,
    Total,
    Video,
)
from cozy_runtime.media import ImageFile, Latents, VideoFile

app = App()
SIDE = 16


class Beat(msgspec.Struct, kw_only=True):
    description: Prompt = Prompt()
    duration: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 5


class Made(Video, kw_only=True):
    """A take, with the latents this package kept of it."""

    latents: Latents | None = None
    own: bool = False


class Picked(Pickup, kw_only=True):
    take: Made
    beats_from: float | None = None


class ShotInput(msgspec.Struct, kw_only=True):
    summary: Prompt = Prompt()
    beats: Annotated[list[Beat], Count(min=1, max=8)]
    turbo: bool = True
    soundscape: Annotated[Prompt, Placeholder("N/A (silent)")] = Prompt()
    music: Annotated[Prompt, Placeholder("N/A (silent)")] = Prompt()
    aspect_ratio: Annotated[AspectRatio, Ratios("16:9")] | None = None
    first_frame: ImageFile | None = None
    last_frame: ImageFile | None = None
    plate: Annotated[VideoFile, Spans("beats")] | None = None
    preset: Literal["quality", "balanced", "fast"] = "balanced"
    steps: Annotated[int, Range(1, 50), Advanced()] | None = None
    seed: Seed | None = None
    pickup: Annotated[Picked, Snap(17, offset=5, first=39)] | None = None


class Shot(msgspec.Struct):
    video: VideoFile
    latents: Latents
    warnings: list[str] = []


def _clip(out: Outputs, level: int) -> VideoFile:
    return out.save_video(np.full((24, SIDE, SIDE, 3), level, np.uint8), fps=24)


@app.tool(label="Shaped Shot", reserved=["d"])
def shot(payload: ShotInput, out: Outputs) -> Shot:
    """A video in its own words, beat by beat."""
    warned = [f"beat {at + 1} says nothing" for at, beat in enumerate(payload.beats) if not beat.description.text]
    latents = out.save_bytes(b"latents of this package" + str(payload.seed).encode(), media_type="application/octet-stream")
    return Shot(_clip(out, 64), latents, warned)


class UpInput(msgspec.Struct, kw_only=True):
    video: Made
    prompt: str = ""
    resolution: Literal["1080p", "1440p"] = "1440p"
    seed: Seed | None = None


class Up(msgspec.Struct):
    video: VideoFile


@app.tool(label="Shaped Up")
def up(payload: UpInput, out: Outputs) -> Up:
    """A video taken up: from its latents when this package made it."""
    return Up(_clip(out, 192))


class PictureInput(msgspec.Struct, kw_only=True):
    prompt: Prompt[Image] = Prompt()
    seed: Seed | None = None


class Still(msgspec.Struct):
    image: ImageFile


def _still(out: Outputs) -> ImageFile:
    return out.save_image(ImageFrame(SIDE, SIDE, b"\x20\x40\x60" * SIDE**2), format="png")


@app.tool(label="Shaped Picture")
def picture(payload: PictureInput, out: Outputs) -> Still:
    """A picture of what it's told, reading each picture it names."""
    return Still(_still(out))
