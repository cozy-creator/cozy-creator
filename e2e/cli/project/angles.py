# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45", "msgspec>=0.19"]
# ///
"""cl-883: an orbit mentioned by angle, as a function gets it. The cell's orbit is one grey a frame, frame n at 5n, so a
picture's grey names its frame and a clip's greys its frames; a prompt says each mention's key and kind. Weightless."""

import msgspec
from cozy_runtime.author import App
from cozy_runtime.fields import Image, Prompt, Video
from cozy_runtime.media import ImageFile, VideoFile

app = App()


class Asked(msgspec.Struct, kw_only=True):
    pictures: list[ImageFile] = []
    clips: list[VideoFile] = []
    prompt: Prompt[Image | Video] = Prompt()


class Seen(msgspec.Struct):
    pictures: list[int]
    clips: list[list[int]]
    keys: list[str]
    kinds: list[str]


def frame(rgb: bytes) -> int:
    """The frame a grey picture is."""
    return round(sum(rgb) / len(rgb) / 5)


@app.entrypoint(accelerator=False)
def angles(payload: Asked) -> Seen:
    """Which of the orbit's frames each input holds."""
    refs = payload.prompt.references
    return Seen(
        pictures=[frame(picture.read().tobytes()) for picture in payload.pictures],
        clips=[[frame(rgb) for rgb in clip.read(audio=False).frames_rgb] for clip in payload.clips],
        keys=list(refs.keys()),
        kinds=[type(ref).__name__.lower() for ref in refs.values()],
    )
