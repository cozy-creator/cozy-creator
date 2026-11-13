# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19", "numpy>=2"]
# ///
"""cl-983's tool: a short grey clip sized as MiniMax H3 sizes its own, its short side 768 and its long one the nearest multiple of 64, so
16:9 is 1344x768 (1.75) and 1.37:1 is 1024x768 (4:3). Weightless: web/ratios.js reads what its box says of each take."""

import msgspec
import numpy as np
from cozy_runtime.author import App, Outputs
from cozy_runtime.fields import AspectRatio, Image, Prompt
from cozy_runtime.media import VideoFile

app = App()
SHORT = 768


class Asked(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    aspect_ratio: AspectRatio | None = None


class Rounded(msgspec.Struct):
    video: VideoFile


@app.tool(label="Rounded Shot", named="shot")
def shot(payload: Asked, out: Outputs) -> Rounded:
    """Its shape's size, rounded to 64 as H3 rounds it: 16:9 unset."""
    width, height = (float(side) for side in (payload.aspect_ratio or "16:9").split(":"))
    ratio = width / height
    long = max(64, round(SHORT * max(ratio, 1 / ratio) / 64) * 64)
    w, h = (long, SHORT) if ratio >= 1 else (SHORT, long)
    return Rounded(out.save_video(np.full((12, h, w, 3), 96, np.uint8), fps=24))
