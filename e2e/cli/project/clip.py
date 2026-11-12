# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19", "numpy>=2", "tomli-w>=1"]
# ///
"""cl-905's video tool: a short clip of the picture its words mention. Its environment is its own (tomli-w), so a box waiting for
that picture has something of its own to prepare meanwhile: this environment, on the machine it will run on."""

import msgspec
import numpy as np
from cozy_runtime.author import App, Outputs
from cozy_runtime.fields import Image, Prompt
from cozy_runtime.media import VideoFile

app = App()
WIDTH, HEIGHT, FRAMES = 96, 64, 24


class ClipBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()


class Clip(msgspec.Struct):
    video: VideoFile


@app.tool(label="Clip", named="clip")
def clip(payload: ClipBox, out: Outputs) -> Clip:
    """A second of grey with a bar crossing it."""
    frames = np.full((FRAMES, HEIGHT, WIDTH, 3), 40, dtype=np.uint8)
    for n in range(FRAMES):
        frames[n][:, n * WIDTH // FRAMES : n * WIDTH // FRAMES + 4] = 230
    return Clip(out.save_video(frames, fps=24))
