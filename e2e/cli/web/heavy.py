# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The localfit cell's package: a still made on the CPU by a function that, as MiniMax H3's shot does, orchestrates a GPU function of its
own that loads a model. That is what a computer with no GPU does poorly, so rendering it there asks first. The still never calls it, so
nothing is fetched."""

import os

import msgspec
from cozy_runtime.author import App, ImageFile, ImageFrame, Outputs, Weights

app = App()
SIDE = 16


class Words(msgspec.Struct, kw_only=True):
    prompt: str
    seed: int | None = None


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


@app.entrypoint(accelerator=False)
def heavy(payload: Words, out: Outputs) -> Picture:
    """A still of one colour."""
    return Picture(out.save_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png"), payload.prompt)


@app.child_task(defaults={"weights": "zhengpeng7/birefnet@0.1.0"})
def paint(payload: Words, weights: Weights, out: Outputs) -> Picture:
    """Never called: the model on a GPU that makes `heavy` one."""
    return heavy(payload, out)
