"""The cli suite's hostile_inputs cell (xs-196 D): a video input's length, as the Runtime's decoder reads it.
Weightless; it runs on any computer."""

import msgspec

from cozy_runtime.author import App
from cozy_runtime.media import VideoFile

app = App()


class Clip(msgspec.Struct):
    clip: VideoFile


class Probed(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False)
def probe(payload: Clip) -> Probed:
    """The clip's length."""
    return Probed(seconds=float(payload.clip.probe().duration))
