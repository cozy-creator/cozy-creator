# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The project cell's shot package: weightless. `shoot` is a tool: its video is the take it picks up from, else its plate, else the first
video its text mentions, and `given` says what it got, its pickup too. `refuse` is its own no. `bare` is a function of just its video,
`scenes` a long shot's way, segment by segment, each shown as it ends, `fresh` makes its plate's video new bytes, a file nothing has
measured, `noise` a video ffprobe can't read, `crash` fails, `slow` waits and has no seed, `takes` waits with one, and `typed` says when a field isn't one."""

import json
import os
import struct
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, InvalidRequest, Outputs, Seed, Telemetry
from cozy_runtime.fields import Pickup, Prompt, Range, Total, Video
from cozy_runtime.media import File, Latents, VideoFile

app = App()


class Beat(msgspec.Struct, kw_only=True):
    description: Prompt = Prompt()
    duration: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 5


class ShotBox(msgspec.Struct, kw_only=True):
    summary: Prompt = Prompt()
    beats: list[Beat] = []
    plate: VideoFile | None = None
    seed: Seed | None = None
    pickup: Pickup | None = None
    refuse: str = ""


class Take(msgspec.Struct):
    video: VideoFile
    latents: Latents
    given: File
    warnings: list[str]


def _said(prompt: Prompt) -> dict[str, object]:
    return {"text": prompt.text, "references": {key: type(value).__name__.lower() for key, value in prompt.references.items()}}


@app.tool(label="Shot", named="shot", reserved=["d", "english"])
def shoot(payload: ShotBox, out: Outputs) -> Take:
    """A video of the take it picks up from, its plate, or the first video it mentions, and what it was given."""
    if payload.refuse:
        raise InvalidRequest(payload.refuse)
    prompts = [payload.summary, *(beat.description for beat in payload.beats)]
    mentioned = [value.file for prompt in prompts for value in prompt.references.values() if isinstance(value, Video)]
    source = payload.pickup.take.file if payload.pickup else payload.plate or next(iter(mentioned), None)
    if source is None:
        raise InvalidRequest("this mentions no video to shoot")
    pickup = payload.pickup
    given = {
        "summary": _said(payload.summary),
        "beats": [_said(beat.description) for beat in payload.beats],
        "plate": payload.plate is not None,
        "seed": payload.seed,
        "pickup": {"out_frame": pickup.out_frame, "description": pickup.take.description} if pickup else None,
    }
    with source.stream() as events:
        video = out.save_video_stream(iter(events)).video
    return Take(video, out.save_bytes(os.urandom(32), media_type="application/octet-stream"), out.save_bytes(json.dumps(given).encode(), media_type="application/json"), [])


class Bare(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: Prompt = Prompt()


class BareTake(msgspec.Struct):
    video: File


class Fresh(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: Prompt = Prompt()
    seed: int | None = None


class Scenes(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: Prompt = Prompt()
    segments: int
    each: float = 2
    seed: int | None = None


@app.entrypoint()
def scenes(payload: Scenes, out: Outputs, tel: Telemetry) -> BareTake:
    """A long shot's way: its segments in turn, each in counted steps and shown as it ends, and a warning any shot gives alike."""
    tel.log("the fixture's lens is dusty", level="warn")
    (count, steps, video) = (payload.segments, 5, None)
    for index in range(count):
        with tel.scope(f"Segment {index + 1} of {count}", overall_range=(index / count, (index + 1) / count)):
            step = tel.step_callback(steps, stage="denoise")
            for at in range(steps):
                time.sleep(payload.each / steps)
                step(at)
        video = out.save_bytes(payload.plate.read_bytes() + struct.pack(">I4s", 24, b"free") + os.urandom(16), media_type="video/mp4")
        out.publish("video", video, label="Video" if index + 1 == count else f"Segment {index + 1} of {count}")
    return BareTake(video)


@app.entrypoint()
def bare(payload: Bare, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(payload.plate.read_bytes(), media_type="video/mp4"))


@app.entrypoint()
def fresh(payload: Fresh, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(payload.plate.read_bytes() + struct.pack(">I4s", 24, b"free") + os.urandom(16), media_type="video/mp4"))


class Words(msgspec.Struct, kw_only=True):
    summary: str = ""


class Text(msgspec.Struct, kw_only=True):
    prompt: str


class Saved(msgspec.Struct):
    given: File


class Noise(msgspec.Struct, kw_only=True):
    summary: Prompt = Prompt()


class Crash(msgspec.Struct, kw_only=True):
    crash: str
    summary: Prompt = Prompt()


@app.entrypoint()
def noise(payload: Noise, out: Outputs) -> BareTake:
    return BareTake(out.save_bytes(os.urandom(4096), media_type="video/mp4"))


@app.entrypoint()
def crash(payload: Crash, out: Outputs) -> BareTake:
    raise RuntimeError(payload.crash)


@app.entrypoint()
def typed(payload: Words, out: Outputs) -> Saved:
    return Saved(out.save_bytes(payload.summary.encode() + os.urandom(4), media_type="application/json"))


class Untyped(msgspec.Struct):
    blob: File


@app.entrypoint()
def only_untyped(payload: Text, out: Outputs) -> Untyped:
    return Untyped(out.save_bytes(payload.prompt.encode() + os.urandom(4), media_type="application/octet-stream"))


class Echo(msgspec.Struct, kw_only=True):
    b: str = ""
    c: int = 0


@app.entrypoint()
def echo(payload: Echo, out: Outputs) -> Saved:
    return Saved(out.save_bytes(json.dumps(msgspec.to_builtins(payload), sort_keys=True).encode(), media_type="application/json"))


class Wait(msgspec.Struct, kw_only=True):
    seconds: float = 20
    prompt: str = ""


@app.entrypoint()
def slow(payload: Wait, out: Outputs) -> BareTake:
    time.sleep(payload.seconds)
    return BareTake(out.save_bytes(os.urandom(8), media_type="video/mp4"))


class Takes(msgspec.Struct, kw_only=True):
    seconds: float = 20
    seed: int | None = None


@app.entrypoint()
def takes(payload: Takes, out: Outputs) -> BareTake:
    time.sleep(payload.seconds)
    return BareTake(out.save_bytes(os.urandom(8), media_type="video/mp4"))
