# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.38", "msgspec>=0.19", "numpy>=2"]
# ///
"""The client-gone session's local renderer: no GPU, no weights. `make` works for `seconds`, then saves a picture of noise `side`
pixels across, so its landing takes as long as the link it crosses. An asset that mentions another reads that one's file by its
digest in `read`, which says how many bytes it found. `film` is a clip of noise with silence under it, for a cut and a frame."""

import os
import time

import msgspec
import numpy as np

from cozy_runtime.author import App, ImageFile, ImageFrame, ImageResult, Invocation, Operation, Outputs, VideoFile

app = App()


class Make(msgspec.Struct):
    seconds: float = 0
    side: int = 512
    prompt: str = ""


class Read(msgspec.Struct):
    image: ImageFile


class Tiny(msgspec.Struct):
    image: ImageFile
    read_bytes: int


@app.entrypoint()
def make(payload: Make, out: Outputs) -> ImageResult:
    time.sleep(payload.seconds)
    side = payload.side
    return ImageResult(out.save_image(ImageFrame(side, side, os.urandom(side * side * 3)), format="png"), side, side, 0, payload.prompt)


@app.entrypoint()
def read(payload: Read, out: Outputs) -> Tiny:
    held = payload.image.read()
    tiny = out.save_image(ImageFrame(8, 8, os.urandom(8 * 8 * 3)), format="png")
    return Tiny(tiny, held.width * held.height)


class Film(msgspec.Struct):
    seconds: float = 8
    prompt: str = ""


class Filmed(msgspec.Struct):
    video: VideoFile


@app.entrypoint()
def film(payload: Film, out: Outputs) -> Filmed:
    frames = int(payload.seconds * 24)
    noise = np.random.randint(0, 255, (frames, 360, 640, 3), dtype=np.uint8)
    silence = np.zeros((2, int(payload.seconds * 48000)), dtype=np.float32)
    return Filmed(out.save_video(noise, fps=24, audio=silence, sample_rate=48000))


@app.project_adapter("video")
def adapt_video(op: Operation) -> list[Invocation]:
    seconds = {"seconds": op.fields["seconds"]} if "seconds" in op.fields else {}
    return [Invocation(function="film", input={"prompt": op.asset, **seconds})]


@app.project_adapter("image")
def adapt(op: Operation) -> list[Invocation]:
    for found in op.reads:
        if files := found.files_of("image"):
            return [Invocation(function="read", input={"image": files[0].sha256})]
    fields = {field: op.fields[field] for field in ("seconds", "side") if field in op.fields}
    return [Invocation(function="make", input={"prompt": op.asset, **fields})]
