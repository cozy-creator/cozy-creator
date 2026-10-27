# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]
# ///
"""The cli suite's image-ratio script: a weightless image request whose reply is the canvas the shared
`ImageRequest` contract resolves. A ratio outside 1:16 through 16:1 is refused while the request decodes."""

import msgspec

from cozy_runtime.author import Adjustments, App, ImageRequest, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the ratio script's 0.25-4 MP")


class Canvas(msgspec.Struct):
    width: int
    height: int


@app.entrypoint()
def canvas(payload: ImageRequest, adj: Adjustments) -> Canvas:
    width, height = payload.size(adj, None, SIZES, megapixels=1.0)
    return Canvas(width=width, height=height)
