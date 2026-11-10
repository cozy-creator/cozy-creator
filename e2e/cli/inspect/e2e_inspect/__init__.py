"""The cli suite's bound run (cl-716 B): what a function was given, as Calcifer bound it: a video clip's length, a
picture's bytes and a folder's files. Weightless; it runs on any computer."""

import msgspec

from cozy_runtime.author import App
from cozy_runtime.media import ImageFile, Tree, VideoFile

app = App()


class Inspect(msgspec.Struct):
    clip: VideoFile
    still: ImageFile
    folder: Tree
    label: str = "inspect"


class Inspected(msgspec.Struct):
    label: str
    clip_seconds: float
    clip_bytes: int
    still_bytes: int
    folder_files: list[str]


@app.entrypoint(accelerator=False)
def inspect(payload: Inspect) -> Inspected:
    """Says what it was given."""
    root = payload.folder.path
    return Inspected(
        label=payload.label,
        clip_seconds=float(payload.clip.probe().duration),
        clip_bytes=payload.clip.size_bytes,
        still_bytes=payload.still.size_bytes,
        folder_files=sorted(str(p.relative_to(root)) for p in payload.folder.files()),
    )
