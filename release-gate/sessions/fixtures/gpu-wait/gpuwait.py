# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.46", "msgspec>=0.19"]
# ///
"""A real wait for a GPU, on any machine with one (cl-1197; sessions/gpu-wait.sh, and whatever else needs one staged): `hold` takes the
GPU `seconds`, then draws a still. `wait`, rendered meanwhile, is under way on the CPU and calls `turn` on the GPU, as MiniMax H3's shot
calls its segment (gpu-queue's `late`): its run stays in its own stage, "composing", while its call waits for that GPU, the wait Paul
read as stuck (PRO6-61). A plain GPU tool would not do: not started, it only stands in line, "Queued, 1st in line". Each GPU function
builds a model of its own, never reading it (as gpu-queue's), so each is a construction of its own and takes the GPU for its turn
alone. Both are tools, so a box renders them and its face shows the wait."""

import os
import time
from typing import Annotated

import msgspec
from cozy_runtime.author import App, ImageFrame, Loader, Model, Seed, Telemetry, call
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Extra(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Held(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    seconds: Annotated[float, Range(0, 900, unit="s")] = 0
    seed: Seed | None = None


def still(payload: Held) -> ImageResult:
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.instructions.text)


@app.tool(label="Hold the GPU, then draw", named="hold", defaults={"extra": "zhengpeng7/birefnet@0.1.0"})
def hold(payload: Held, extra: Extra, tel: Telemetry) -> ImageResult:
    with tel.stage("holding"):
        time.sleep(payload.seconds)
    return still(payload)


class Turn(msgspec.Struct, kw_only=True):
    seconds: float = 0


class Took(msgspec.Struct):
    done_ms: int


@app.entrypoint(private=True, accelerator=True, defaults={"extra": "madebyollin/texture-fix-vae@0.3.0"})
def turn(payload: Turn, extra: Extra) -> Took:
    return Took(int(time.time() * 1000))


@app.tool(label="Wait for the GPU, then draw", named="wait", accelerator=False)
async def wait(payload: Held, tel: Telemetry) -> ImageResult:
    with tel.stage("composing"):
        await call(turn, Turn())
    return still(payload)
