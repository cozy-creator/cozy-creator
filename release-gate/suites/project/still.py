# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project suite's local renderer (xs-139): a weightless still at the canvas the request asks for, one colour from
the prompt, so a project renders an asset `tool: ./still.py/draw` from code on this computer. `keep` answers with the picture it's
given, so a picture counts as this package's without being drawn again."""

import hashlib

import msgspec
from cozy_runtime.author import Adjustments, App, ImageFrame, Outputs
from cozy_runtime.image import ImageRequest, ImageResult, Sizes
from cozy_runtime.media import ImageFile

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


@app.entrypoint()
def draw(payload: ImageRequest, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = payload.size(adj, None, SIZES, megapixels=1.0, ratio="16:9")
    rgb = hashlib.sha256(payload.prompt.encode()).digest()[:3]
    image = out.save_image(ImageFrame(width, height, rgb * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)


class Kept(msgspec.Struct, kw_only=True):
    image: ImageFile


@app.entrypoint()
def keep(payload: Kept, out: Outputs) -> Kept:
    return Kept(image=out.save_bytes(payload.image.read_bytes(), media_type="image/png"))
