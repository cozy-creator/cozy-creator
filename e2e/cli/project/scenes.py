# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19"]
# ///
"""The timed-list cell's tool (xs-215): a video box whose timed list is `scenes`, each a length that adds up (`Total()`) and plain words, not
H3's `beats` of prompts. Weightless: its video is its plate's."""

from typing import Annotated

import msgspec
from cozy_runtime.author import App, InvalidRequest, Outputs
from cozy_runtime.fields import Image, Prompt, Range, Total, Video
from cozy_runtime.media import VideoFile

app = App()


class Scene(msgspec.Struct, kw_only=True):
    seconds: Annotated[float, Range(1, 15, step=1, unit="s"), Total()] = 5
    look: str = ""


class SceneBox(msgspec.Struct, kw_only=True):
    summary: Prompt[Image | Video] = Prompt()
    scenes: list[Scene] = []
    plate: VideoFile | None = None


class Made(msgspec.Struct):
    video: VideoFile


@app.tool(label="Scenes", named="scenes")
def scene(payload: SceneBox, out: Outputs) -> Made:
    """Its plate, as given."""
    if payload.plate is None:
        raise InvalidRequest("this has no plate")
    with payload.plate.stream() as events:
        return Made(out.save_video_stream(iter(events)).video)
