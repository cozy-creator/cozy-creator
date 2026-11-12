# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19", "numpy>=2"]
# ///
"""The client-gone session's local renderer: no GPU, no weights. `make` works for `seconds`, then saves a picture of noise `side`
pixels across, so its landing takes as long as the link it crosses. A prompt that mentions a picture is that picture read instead:
how many bytes it found. `film` is a clip of noise with silence under it, for a cut and a frame."""

import os
import time

import msgspec
import numpy as np

from cozy_runtime.author import App, ImageFrame, Outputs
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import ImageFile, VideoFile

app = App()


class Make(msgspec.Struct, kw_only=True):
    seconds: float = 0
    side: int = 512
    prompt: Prompt[Image] = Prompt()


class Made(msgspec.Struct):
    image: ImageFile
    width: int
    read_bytes: int = 0


@app.entrypoint()
def make(payload: Make, out: Outputs) -> Made:
    picture = next(iter(payload.prompt.references.values()), None)
    if picture is not None:
        held = picture.file.read()
        return Made(out.save_image(ImageFrame(8, 8, os.urandom(8 * 8 * 3)), format="png"), 8, held.width * held.height)
    time.sleep(payload.seconds)
    side = payload.side
    return Made(out.save_image(ImageFrame(side, side, os.urandom(side * side * 3)), format="png"), side)


class Film(msgspec.Struct):
    seconds: float = 8
    prompt: str = ""


class Filmed(msgspec.Struct):
    video: VideoFile


@app.entrypoint()
def film(payload: Film, out: Outputs) -> Filmed:
    frames = int(payload.seconds * 24)
    noise = np.random.default_rng().integers(0, 255, (frames, 360, 640, 3), dtype=np.uint8)  # its own generator: the contract refuses the global one
    silence = np.zeros((2, int(payload.seconds * 48000)), dtype=np.float32)
    return Filmed(out.save_video(noise, fps=24, audio=silence, sample_rate=48000))
