# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's shot package: weightless, and each function is a node. `shoot`'s video is its first mention's, or its plate, and
`given` says what it got. `refuse` is its own no. `bare` is a function of just its video, `scenes` a long shot's way, segment by segment,
each shown as it ends, `fresh` makes its plate's video new bytes, a file nothing has measured, `noise` a video ffprobe can't read,
`crash` fails, `slow` waits and has no seed, and `typed` says when a field isn't one."""

import json
import os
import struct
import time

import msgspec
from cozy_runtime.author import App, File, InvalidRequest, Node, Outputs, Telemetry, VideoFile

app = App()


class Plate(msgspec.Struct, kw_only=True):
    video: File


class Keeping(msgspec.Struct, kw_only=True):
    video: File
    latents: File | None = None
    at_s: float | None = None


class Beat(msgspec.Struct, kw_only=True):
    description: str = ""
    duration: float = 5


class Seen(msgspec.Struct, kw_only=True):
    name: str
    video: File | None = None
    image: File | None = None
    latents: File | None = None


class Request(msgspec.Struct, kw_only=True):
    prompt: str = ""
    summary: str = ""
    beats: list[Beat] = []
    plate: Plate | None = None
    references: list[Seen] = []
    seed: int | None = None
    continue_from: Keeping | None = None
    refuse: str = ""


class Take(msgspec.Struct):
    video: File
    latents: File
    given: File
    kept_s: float | None
    warnings: list[str]


@app.entrypoint(node=Node(reserved=["d", "english"], fill={"continue_from": "take"}))
def shoot(payload: Request, out: Outputs) -> Take:
    if payload.refuse:
        raise InvalidRequest(payload.refuse)
    keeps = payload.continue_from
    plate = payload.plate.video if payload.plate else next((seen.video for seen in payload.references if seen.video), None)
    if plate is None:
        raise InvalidRequest("this mentions no video to shoot")
    said = [f"{seen.name}={'video' if seen.video else 'image'}{'+latents' if seen.latents else ''}" for seen in payload.references]
    given = {"prompt": " | ".join([payload.prompt or payload.summary, *said]), "seed": payload.seed, "keep": keeps.at_s if keeps else None}
    return Take(out.save_bytes(plate.read_bytes(), media_type="video/mp4"), out.save_bytes(os.urandom(32), media_type="application/octet-stream"),
                out.save_bytes(json.dumps(given).encode(), media_type="application/json"),
                (keeps.at_s or 0) + 0.04 if keeps else None, [])


class Bare(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: str = ""


class BareTake(msgspec.Struct):
    video: File


class Fresh(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: str = ""
    seed: int | None = None


class Scenes(msgspec.Struct, kw_only=True):
    plate: VideoFile
    summary: str = ""
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
    summary: str = ""


class Crash(msgspec.Struct, kw_only=True):
    crash: str
    summary: str = ""


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
