# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""The project suite's local renderer (xs-139): a weightless still at the canvas the request asks for, one colour from
the prompt, so a project renders a node `with: ./still.py/draw` from code on this computer."""

import hashlib

from cozy_runtime.author import Adjustments, App, ImageFrame, ImageRequest, ImageResult, Outputs, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


@app.entrypoint()
def draw(payload: ImageRequest, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = payload.size(adj, None, SIZES, megapixels=1.0)
    rgb = hashlib.sha256(payload.prompt.encode()).digest()[:3]
    image = out.save_image(ImageFrame(width, height, rgb * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)
