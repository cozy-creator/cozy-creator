# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19", "numpy>=2"]
# ///
"""xs-180's tool: one whole turn of a drawn marker, every frame a keyframe, its file declared an orbit (`File.with_metadata`), as the
`orbit` package declares its H3 clips. Weightless: a video node shows it turnable on any computer."""

from typing import Annotated, Literal

import msgspec
import numpy as np
from cozy_runtime.author import App, Outputs
from cozy_runtime.fields import Range
from cozy_runtime.media import VideoFile

app = App()
WIDTH, HEIGHT = 96, 64


class TurnBox(msgspec.Struct, kw_only=True):
    frames: Annotated[int, Range(8, 96)] = 48
    direction: Literal["cw", "ccw"] = "cw"


class Turned(msgspec.Struct):
    video: VideoFile


@app.tool(label="Turn", named="turn")
def turn(payload: TurnBox, out: Outputs) -> Turned:
    """A marker going once round: its frame `n` is the angle `n / frames × 360`, red in front, blue behind."""
    count = payload.frames
    x = np.arange(WIDTH)
    frames = np.full((count, HEIGHT, WIDTH, 3), 40, dtype=np.uint8)
    for n in range(count):
        angle = 2 * np.pi * n / count
        bar = np.abs(x - (WIDTH / 2 + np.sin(angle) * WIDTH / 3)) < 6
        front = bool(np.cos(angle) > 0)
        frames[n][:, bar] = (230, 60, 40) if front else (40, 80, 220)
        frames[n][: HEIGHT * n // count, :4] = 255
    # Runtime 0.45.0, which the type check pins, predates both: the candidate Runtime has them.
    video = out.save_video(frames, fps=24, keyframe_interval=1)  # type: ignore[call-arg]
    orbit = {"orbit": {"degrees": 360, "direction": payload.direction, "loops": True}}
    return Turned(video.with_metadata(orbit))  # type: ignore[attr-defined]
