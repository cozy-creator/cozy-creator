"""The cli suite's bound run (cl-716 B): what a function was given, as Calcifer bound it: a video clip's length, a
picture's bytes, a folder's files, and the union fields a package's smoke request names as text (`aspect_ratio=1:1`,
`seed=7`: 0.22.1's binder refused them). Weightless; it runs on any computer."""

import msgspec

from cozy_runtime.author import App, AspectRatio, ImageFile, Seed, Tree, VideoFile

app = App()


class Inspect(msgspec.Struct):
    clip: VideoFile
    still: ImageFile
    folder: Tree
    label: str = "inspect"
    aspect_ratio: AspectRatio | None = None
    seed: Seed | None = None


class Inspected(msgspec.Struct):
    label: str
    clip_seconds: float
    clip_bytes: int
    still_bytes: int
    folder_files: list[str]
    aspect_ratio: str | None
    seed: int | None


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
        aspect_ratio=None if payload.aspect_ratio is None else str(payload.aspect_ratio),
        seed=None if payload.seed is None else int(payload.seed),
    )
