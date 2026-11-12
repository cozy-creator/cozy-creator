# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The node cell's package: weightless tools in a video model's own shape, so what a project gives a function can be read from its run
with no model. `shot` takes a summary and beats, a first and last frame, a plate and a pickup. `up` is an upscale whose video slot reads
the latents of a take this package made. Each makes a short grey clip of its own. `picture` takes pictures by its prompt."""

import json
from typing import Annotated, Literal, cast

import msgspec
import numpy as np
from cozy_runtime.author import App, ImageFrame, InvalidRequest, Outputs, Seed
from cozy_runtime.fields import (
    Advanced,
    AspectRatio,
    Audio,
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
    description: Prompt[Image | Video | Audio] = Prompt()
    duration: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 5


class Made(Video, kw_only=True, frozen=True):
    """A take, with the latents this package kept of it."""

    latents: Latents | None = None
    own: bool = False


class Picked(Pickup, kw_only=True, frozen=True):
    take: Made
    beats_from: float | None = None


class ShotInput(msgspec.Struct, kw_only=True):
    summary: Prompt[Image | Video | Audio] = Prompt()
    beats: Annotated[list[Beat], Count(min=1, max=8)]
    turbo: bool = True
    soundscape: Annotated[Prompt[Image | Video | Audio], Placeholder("N/A (silent)")] = Prompt()
    music: Annotated[Prompt[Image | Video | Audio], Placeholder("N/A (silent)")] = Prompt()
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
    #: Where each segment after the first starts, as H3 says them (se-130): segments of 39, 34 and 34 frames.
    seams: list[int] = []


#: Its takes: 107 frames, on its grid, in three segments; their seams are in their latents, as H3 keeps them.
FRAMES, SEAMS = 107, (39, 73)


def _held(take: Made) -> list[int] | None:
    try:
        return [int(seam) for seam in json.loads(take.latents.read_bytes())["seams"]] if take.latents is not None else None
    except (ValueError, KeyError, TypeError):
        return None


def _clip(out: Outputs, level: int, frames: int = 24) -> VideoFile:
    return out.save_video(np.full((frames, SIDE, SIDE, 3), level, np.uint8), fps=24)


@app.tool(label="Shaped Shot", reserved=["d"])
def shot(payload: ShotInput, out: Outputs) -> Shot:
    """A video in its own words, beat by beat."""
    warned = [f"beat {at + 1} says nothing" for at, beat in enumerate(payload.beats) if not beat.description.text]
    seams = list(SEAMS)
    # H3's contract (se-130): a pickup starts at its take's seam at or before its frame, saying so when that moved it; every
    # segment before it stays, and the rest goes again on the shot's own plan.
    if payload.pickup is not None and (held := _held(payload.pickup.take)) is not None:
        asked = payload.pickup.out_frame
        cut = max((seam for seam in [*held, FRAMES] if seam <= asked), default=None) if asked < FRAMES else FRAMES
        if cut is None:
            raise InvalidRequest(f"picking up at frame {asked} keeps nothing: render without a pickup to make it all again", fields=["pickup.out_frame"])
        if cut != asked:
            warned.append(f"a pickup starts where a segment does, so this keeps up to frame {cut}, not {asked}")
        seams = sorted(seam for seam in {*(seam for seam in held if seam < cut), cut, *(seam for seam in SEAMS if seam > cut)} if 0 < seam < FRAMES)
    latents = out.save_bytes(json.dumps({"seams": seams, "seed": payload.seed}).encode(), media_type="application/octet-stream")
    return Shot(_clip(out, 64, FRAMES), cast(Latents, latents), warned, seams)  # a saved file fills a Latents field: the Runtime checks its kind


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
