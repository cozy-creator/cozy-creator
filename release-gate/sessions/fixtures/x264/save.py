# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.47", "msgspec>=0.19", "numpy>=1.26"]
# ///
"""x264-threads' clip (xs-220, hack audit runtime #8): `frames` frames of 1344x768 saved as one mp4, with the x264 thread count
the Runtime chose on this host, the CPUs this process may use, and the save's seconds. The thread count is read from the
saved file itself: x264 writes its settings (`… threads=N lookahead_threads=M …`) into the stream. `arm` only names the call,
so the two arms of a session are two computations, never one memo."""

import os
import re
import time

import msgspec
import numpy as np

from cozy_runtime.author import App, Outputs, VideoFile

app = App()


class Clip(msgspec.Struct, forbid_unknown_fields=True):
    arm: str
    frames: int = 360


class Saved(msgspec.Struct):
    video: VideoFile
    threads: int
    cpus: int
    save_s: float


@app.entrypoint()
def save(payload: Clip, out: Outputs) -> Saved:
    y, x = np.mgrid[0:768, 0:1344].astype(np.uint16)
    frames = np.empty((payload.frames, 768, 1344, 3), np.uint8)
    for i in range(payload.frames):
        frames[i, ..., 0] = (x + 3 * i) & 255
        frames[i, ..., 1] = (y + 5 * i) & 255
        frames[i, ..., 2] = (x ^ y) + i & 255
    start = time.perf_counter()
    video = out.save_video(frames, fps=24)
    save_s = time.perf_counter() - start
    said = re.search(rb" threads=(\d+)", video.read_bytes())
    if said is None:
        raise RuntimeError("the saved mp4 carries no x264 settings")
    return Saved(video=video, threads=int(said.group(1)), cpus=len(os.sched_getaffinity(0)), save_s=save_s)
