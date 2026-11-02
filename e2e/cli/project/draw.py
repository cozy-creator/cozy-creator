# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""The project cell's renderer: a weightless still of one random colour, so each version differs."""

import os

from cozy_runtime.author import Adjustments, App, ImageFrame, ImageRequest, ImageResult, Outputs, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


@app.entrypoint()
def draw(payload: ImageRequest, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = payload.size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)
