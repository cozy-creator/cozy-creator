# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The kept-latents cell's package (cl-1046): `shot` makes a short clip and latents of `size` bytes, as a video model keeps its
latents; `read` reads a file of its project whole, named by its hash, and says its digest."""

import hashlib
import os
from typing import cast

import msgspec
import numpy as np
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File, Latents, VideoFile

app = App()
SIDE = 16


class ShotInput(msgspec.Struct, kw_only=True):
    size: int = 64


class Shot(msgspec.Struct):
    video: VideoFile
    latents: Latents


@app.tool(label="Kept Shot")
def shot(payload: ShotInput, out: Outputs) -> Shot:
    """A short clip, and latents of `size` bytes."""
    video = out.save_video(np.full((24, SIDE, SIDE, 3), 64, np.uint8), fps=24)
    latents = out.save_bytes(os.urandom(payload.size), media_type="application/octet-stream")
    return Shot(video, cast(Latents, latents))  # a saved file fills a Latents field: the Runtime checks its kind


class ReadInput(msgspec.Struct, kw_only=True):
    data: File


class Read(msgspec.Struct):
    digest: str


@app.entrypoint(accelerator=False)
def read(payload: ReadInput) -> Read:
    """The file's digest, read whole."""
    return Read("sha256:" + hashlib.sha256(payload.data.read_bytes()).hexdigest())
