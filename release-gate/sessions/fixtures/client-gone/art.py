# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The client-gone session's local renderer: no GPU, no weights. `make` works for `seconds`, then saves a picture of noise `side`
pixels across, so its landing takes as long as the link it crosses. An asset that mentions another reads that one's file by its
digest in `read`, which says how many bytes it found."""

import os
import time

import msgspec

from cozy_runtime.author import App, ImageFile, ImageFrame, ImageResult, Invocation, Operation, Outputs

app = App()


class Make(msgspec.Struct):
    seconds: float = 0
    side: int = 512
    prompt: str = ""


class Read(msgspec.Struct):
    image: ImageFile


class Tiny(msgspec.Struct):
    image: ImageFile
    read_bytes: int


@app.entrypoint()
def make(payload: Make, out: Outputs) -> ImageResult:
    time.sleep(payload.seconds)
    side = payload.side
    return ImageResult(out.save_image(ImageFrame(side, side, os.urandom(side * side * 3)), format="png"), side, side, 0, payload.prompt)


@app.entrypoint()
def read(payload: Read, out: Outputs) -> Tiny:
    held = payload.image.read()
    tiny = out.save_image(ImageFrame(8, 8, os.urandom(8 * 8 * 3)), format="png")
    return Tiny(tiny, held.width * held.height)


@app.project_adapter("image")
def adapt(op: Operation) -> list[Invocation]:
    for found in op.reads:
        if files := found.files_of("image"):
            return [Invocation(function="read", input={"image": files[0].sha256})]
    fields = {field: op.fields[field] for field in ("seconds", "side") if field in op.fields}
    return [Invocation(function="make", input={"prompt": op.asset, **fields})]
