# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43", "msgspec>=0.19"]
# ///
"""The tools cells' every-type fixture (web/tools.js, and the binder's weightless cell 1): weightless tools whose boxes hold every type the SDK
has, groups two deep, so the page draws each and each reaches the function as typed. `shot` makes a video of its plate's bytes, or of the take it picks up from, shows each segment as it ends, and says in
`given` what it got. `still` makes a picture of one random colour."""

import os
import time
from collections.abc import Mapping
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, ImageFrame, InvalidRequest, Outputs, Seed
from cozy_runtime.fields import Advanced, AspectRatio, Count, Image, Label, Pickup, Placeholder, Prompt, Range, Ratios, Snap, Spans, Total, Video
from cozy_runtime.media import AudioFile, File, ImageFile, Latents, VideoFile

app = App()
SIDE = 16


class Look(msgspec.Struct, kw_only=True):
    """A block in a beat."""

    lens: Literal["wide", "close"] = "wide"
    note: str = ""


class Angle(msgspec.Struct, kw_only=True):
    """A group two deep: a beat's angles."""

    framing: Prompt = Prompt()
    hold: Annotated[float, Range(0.5, 4, step=0.5, unit="s")] = 1.0


class Beat(msgspec.Struct, kw_only=True):
    duration: Annotated[int, Range(1, 15, unit="s"), Total()] = 5
    description: Prompt = Prompt()
    look: Look | None = None
    angles: Annotated[list[Angle], Count(max=3)] = []
    still: ImageFile | None = None


class Everything(msgspec.Struct, kw_only=True):
    summary: Annotated[Prompt, Placeholder("What happens")] = Prompt()
    pictures: Prompt[Image] = Prompt()
    negative: str = ""
    beats: Annotated[list[Beat], Count(min=1, max=4)]
    aspect_ratio: Annotated[AspectRatio, Ratios("16:9", "1:1", "9:16")] | None = None
    turbo: bool = True
    bright: bool | None = None
    megapixels: Annotated[float, Range(0.5, 4, step=0.5, unit="MP")] | None = None
    segments: Annotated[int, Range(1, 4)] = 1
    quality: Literal["draft", "standard", "full"] = "standard"
    resolution: Literal["480p", "720p", "1080p", "1440p", "2160p"] = "720p"
    edit: Annotated[
        Literal["character", "head", "style"],
        Label("Edit", options={"character": ("Character swap", "The reference in place of the person"), "head": ("Head swap", "Only the head"), "style": ("Style transfer", "The reference's look over the whole plate")}),
    ] = "character"
    first_frame: ImageFile | None = None
    plate: Annotated[VideoFile, Spans("beats")] | None = None
    sound: AudioFile | None = None
    clip: Video | None = None
    references: list[Image] = []
    steps: Annotated[int, Range(1, 50), Advanced()] = 8
    seed: Seed | None = None
    pickup: Annotated[Pickup, Snap(5, offset=2, first=2)] | None = None


class Take(msgspec.Struct):
    video: VideoFile
    latents: Latents
    given: File
    warnings: list[str]


def _seen(value: object) -> object:
    """What arrived, as JSON: a file as its type, so the check reads the shape, never bytes."""
    if isinstance(value, (ImageFile, VideoFile, AudioFile, File)):
        return {"file": type(value).__name__}
    if isinstance(value, msgspec.Struct):
        return {name: _seen(getattr(value, name)) for name in value.__struct_fields__}
    if isinstance(value, Mapping):
        return {str(key): _seen(held) for key, held in value.items()}
    if isinstance(value, (list, tuple)):
        return [_seen(held) for held in value]
    return value


@app.tool(label="Every field", named="shot")
def shot(payload: Everything, out: Outputs) -> Take:
    """A video of its plate, or of the take it picks up from, each segment shown as it ends."""
    source = payload.pickup.take.file if payload.pickup else payload.plate
    if source is None:
        raise InvalidRequest("this has no plate and picks up from no take")
    for index in range(payload.segments):
        time.sleep(1.5)
        out.publish("video", _copy(source, out), label=f"Segment {index + 1} of {payload.segments}")
    given = msgspec.json.encode(_seen(payload))
    return Take(_copy(source, out), out.save_bytes(os.urandom(32), media_type="application/octet-stream"), out.save_bytes(given, media_type="application/json"), [])


def _copy(video: VideoFile, out: Outputs) -> VideoFile:
    """The video again, as a file of this run's."""
    with video.stream() as events:
        return out.save_video_stream(iter(events)).video


class StillBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


class Picture(msgspec.Struct):
    image: ImageFile


@app.tool(label="A still", named="still")
def still(payload: StillBox, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"))
