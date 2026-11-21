# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.47", "msgspec>=0.19", "numpy>=1.26"]
# ///
"""x264-threads' clip (xs-220, hack audit runtime #8): `frames` frames of 1344x768 saved as one mp4, and the CPUs this process
may use, and the seconds the call itself took (the frames made and handed over). The Runtime encodes them after the call
returns (its output phase), so the session reads the rest from outside: the x264 threads from the exported mp4 (x264 writes its
settings, `threads=N` among them, into the stream), and the encode as the run's start to its settling, less the call's own. `arm` only names the call, so the two arms of a session are two computations, never one memo."""

import os
import time

import msgspec
import numpy as np

from cozy_runtime.author import App, Outputs
from cozy_runtime.media import VideoFile

app = App()


class Clip(msgspec.Struct, forbid_unknown_fields=True):
    arm: str
    frames: int = 360


class Saved(msgspec.Struct):
    video: VideoFile
    cpus: int
    made_s: float


@app.entrypoint()
def save(payload: Clip, out: Outputs) -> Saved:
    start = time.perf_counter()
    y, x = np.mgrid[0:768, 0:1344].astype(np.uint16)
    frames = np.empty((payload.frames, 768, 1344, 3), np.uint8)
    for i in range(payload.frames):
        frames[i, ..., 0] = (x + 3 * i) & 255
        frames[i, ..., 1] = (y + 5 * i) & 255
        frames[i, ..., 2] = (x ^ y) + i & 255
    video = out.save_video(frames, fps=24)
    return Saved(video=video, cpus=len(os.sched_getaffinity(0)), made_s=time.perf_counter() - start)
