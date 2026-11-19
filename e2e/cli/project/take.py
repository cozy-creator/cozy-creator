# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""A plain video file as a take (cl-1317), typed as H3's are: `pick` picks one up, `redo` remakes a range of one. Each says the
bytes it read, the take's and its latents', by their hashes."""

import hashlib

import msgspec
from cozy_runtime.author import App, InvalidRequest
from cozy_runtime.fields import Pickup, Video
from cozy_runtime.media import Latents

app = App()


class Take(Video, frozen=True, kw_only=True):
    """A take and its latents when given: H3's `ShotVideo`."""

    latents: Latents | None = None
    own: bool = False


class TakePickup(Pickup, frozen=True, kw_only=True):
    take: Take


class Pick(msgspec.Struct, kw_only=True):
    pickup: TakePickup | None = None


class Redo(msgspec.Struct, kw_only=True):
    video: Take
    start_s: float = 0.0


class Read(msgspec.Struct):
    take: str
    latents: str
    out_frame: int


def _read(take: Take, out_frame: int) -> Read:
    latents = "" if take.latents is None else hashlib.sha256(take.latents.read_bytes()).hexdigest()
    return Read(hashlib.sha256(take.file.read_bytes()).hexdigest(), latents, out_frame)


@app.entrypoint(accelerator=False)
def pick(payload: Pick) -> Read:
    """The take it picks up and the frame."""
    if payload.pickup is None:
        raise InvalidRequest("no pickup reached it")
    return _read(payload.pickup.take, payload.pickup.out_frame)


@app.entrypoint(accelerator=False)
def redo(payload: Redo) -> Read:
    """The take it remakes part of."""
    return _read(payload.video, -1)
