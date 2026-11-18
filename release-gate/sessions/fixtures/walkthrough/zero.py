# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""walkthrough.sh's $0 tools, run on a CPU: `seeded` a memoized picture whose colour is its seed's (a seeded model's stand-in), `slow`
a still that takes its time in ten steps, saying how far it's got."""

import hashlib
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, Context, ImageFrame, Seed, Telemetry
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()
SIDE = 64


class SeededBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seed: Seed | None = None


class SlowBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seconds: Annotated[float, Range(0, 600)] = 40.0
    seed: Seed | None = None


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


def _still(seed: object) -> ImageFile:
    return ImageFile.from_image(ImageFrame(SIDE, SIDE, hashlib.sha256(str(seed).encode()).digest()[:3] * SIDE**2), format="png")


@app.tool(label="A seeded still", memoize=True)
def seeded(payload: SeededBox) -> Picture:
    """A still whose colour is its seed's."""
    return Picture(_still(payload.seed), payload.instructions.text)


@app.tool(label="A slow still")
def slow(ctx: Context, payload: SlowBox, tel: Telemetry) -> Picture:
    """A still that takes its time, saying how far it's got in steps."""
    with tel.stage("drawing"):
        for at in range(10):
            time.sleep(payload.seconds / 10)
            ctx.progress(at + 1, 10)
    return Picture(_still(payload.seed), payload.instructions.text)
