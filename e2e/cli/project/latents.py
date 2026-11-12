# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.43.1", "msgspec>=0.19", "torch>=2.5"]
# ///
"""The previews cells' tool (cr-282): a still made as a denoise loop makes one, on the CPU and weightless. Its latents
come out of noise step by step, and each step's clean estimate is shown to a client watching the run (`preview`)."""

import time
from typing import Annotated

import msgspec
import torch
from cozy_runtime.author import App, Context, ImageFrame, Outputs, Telemetry, preview
from cozy_runtime.fields import Image, Prompt, Range
from cozy_runtime.media import ImageFile

app = App()
#: The latents' side: a 512 px picture's, in SDXL's space.
SIDE = 64


class LatentsBox(msgspec.Struct, kw_only=True):
    instructions: Prompt[Image] = Prompt()
    steps: Annotated[int, Range(1, 64)] = 16
    seconds: Annotated[float, Range(0, 10)] = 0.5


class Picture(msgspec.Struct):
    image: ImageFile
    prompt: str


@app.tool(label="A denoised still", named="latents")
def latents(ctx: Context, payload: LatentsBox, out: Outputs, tel: Telemetry) -> Picture:
    """A still whose latents come out of noise over `steps`, `seconds` each, each step's estimate shown."""
    generator = torch.Generator().manual_seed(282)
    clean = torch.randn(1, 4, SIDE, SIDE, generator=generator)
    noise = torch.randn(1, 4, SIDE, SIDE, generator=generator)
    with tel.stage("denoise"):
        for i in range(payload.steps):
            time.sleep(payload.seconds)
            sigma = 1 - (i + 1) / payload.steps
            preview(lambda: clean + sigma * (noise - clean) / 2, "sdxl")
            ctx.progress(i, payload.steps)
    pixels = ((clean[0, :3].clamp(-1, 1) + 1) * 127.5).to(torch.uint8).permute(1, 2, 0).contiguous()
    return Picture(out.save_image(ImageFrame(SIDE, SIDE, pixels.numpy().tobytes()), format="png"), payload.instructions.text)
