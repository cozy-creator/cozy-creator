# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The node cell's package: weightless, in a video model's own shape, so what a project gives a function can be read from its run with no
model. `shot` takes a summary and beats, frames, a plate, a take and everything its text mentions. `up` is an upscale filled from what it
reads. `picture` takes named pictures, each with its words. `pair` makes two."""

from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, AudioFile, File, ImageFile, ImageFrame, Node, Outputs, Picture, VideoFile

app = App()
SIDE = 16


class Beat(msgspec.Struct, kw_only=True):
    description: str = ""
    duration: Annotated[float, msgspec.Meta(extra={"min": 1, "max": 15, "total": True, "unit": "s"})] = 5


class Mentioned(msgspec.Struct, kw_only=True):
    name: str
    description: str = ""
    asset_type: str = ""
    image: ImageFile | None = None
    video: VideoFile | None = None
    audio: AudioFile | None = None
    voice: AudioFile | None = None
    use_audio: bool = False
    follow: Literal["exact", "loose"] = "exact"
    alpha: float = 0
    width: int = 0
    made_by: str = ""


class Plated(msgspec.Struct, kw_only=True):
    video: VideoFile
    use_audio: bool = False


class Keeping(msgspec.Struct, kw_only=True):
    video: VideoFile
    latents: File | None = None
    at_s: float | None = None


class ShotInput(msgspec.Struct, kw_only=True):
    summary: str = ""
    beats: Annotated[list[Beat], msgspec.Meta(min_length=1, max_length=8)]
    turbo: bool = True
    soundscape: Annotated[str, msgspec.Meta(extra={"placeholder": "N/A (silent)"})] = ""
    music: Annotated[str, msgspec.Meta(extra={"placeholder": "N/A (silent)"})] = ""
    aspect_ratio: Annotated[str | None, msgspec.Meta(extra={"options": ["16:9"]})] = None
    first_frame: ImageFile | None = None
    last_frame: ImageFile | None = None
    plate: Plated | None = None
    references: list[Mentioned] = []
    continue_from: Keeping | None = None
    preset: Literal["quality", "balanced", "fast"] = "balanced"
    steps: int | None = None
    seed: int | None = None


class Shot(msgspec.Struct):
    video: File
    latents: File
    warnings: list[str] = []


def _video(payload: ShotInput) -> VideoFile:
    found = [payload.plate.video] if payload.plate else []
    found += [ref.video for ref in payload.references if ref.video is not None]
    found += [payload.continue_from.video] if payload.continue_from else []
    return found[0]


@app.entrypoint(node=Node(label="Shaped Shot", show=["soundscape", "music"], hide=["steps", "plate"], fill={"continue_from": "take"}, reserved=["d"]))
def shot(payload: ShotInput, out: Outputs) -> Shot:
    """A video in its own words, beat by beat."""
    warned = [f"beat {at + 1} says nothing" for at, beat in enumerate(payload.beats) if not beat.description]
    latents = out.save_bytes(b"latents of this package" + str(payload.seed).encode(), media_type="application/octet-stream")
    return Shot(out.save_bytes(_video(payload).read_bytes(), media_type="video/mp4"), latents, warned)


class UpInput(msgspec.Struct, kw_only=True):
    video: VideoFile
    latents: File | None = None
    prompt: str = ""
    resolution: Literal["1080p", "1440p"] = "1440p"
    seed: int | None = None


class Up(msgspec.Struct):
    video: File


@app.entrypoint(node=Node(label="Shaped Up", fill={"latents": "video.latents", "prompt": "video.description"}))
def up(payload: UpInput, out: Outputs) -> Up:
    """A video taken up: it reads its latents, and if it can't, its description."""
    return Up(out.save_bytes(payload.video.read_bytes(), media_type="video/mp4"))


class Mentions(Picture, kw_only=True):
    image: ImageFile | None = None


class PictureInput(msgspec.Struct, kw_only=True):
    prompt: str
    pictures: Annotated[list[Mentions], msgspec.Meta(max_length=4)] = []
    seed: int | None = None


class Still(msgspec.Struct):
    image: ImageFile


def _still(out: Outputs) -> ImageFile:
    return out.save_image(ImageFrame(SIDE, SIDE, b"\x20\x40\x60" * SIDE**2), format="png")


@app.entrypoint(node=Node(label="Shaped Picture"))
def picture(payload: PictureInput, out: Outputs) -> Still:
    """A picture of what it's told, reading each it names."""
    return Still(_still(out))


class Nothing(msgspec.Struct, kw_only=True):
    prompt: str = ""


class Pair(msgspec.Struct):
    image: list[ImageFile]


@app.entrypoint(node=Node(label="Shaped Pair"))
def pair(payload: Nothing, out: Outputs) -> Pair:
    """Two pictures, as one asset."""
    return Pair([_still(out), _still(out)])
