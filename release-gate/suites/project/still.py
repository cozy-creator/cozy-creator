# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project suite's local renderer (xs-139): a weightless still at the canvas the request asks for, one colour from
the prompt, so a project renders an asset `with: ./still.py` from code on this computer. With `keep`, it answers no run, so a
picture counts as this package's without being drawn again."""

import hashlib

from cozy_runtime.author import Adjustments, App, ImageFrame, ImageRequest, ImageResult, Invocation, Operation, Outputs, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


@app.entrypoint()
def draw(payload: ImageRequest, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = payload.size(adj, None, SIZES, megapixels=1.0)
    rgb = hashlib.sha256(payload.prompt.encode()).digest()[:3]
    image = out.save_image(ImageFrame(width, height, rgb * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)


@app.project_adapter("image")
def adapt(op: Operation) -> list[Invocation]:
    # `keep`: no call, so the asset's file is the picture it reads, made by this package.
    if op.fields.get("keep"):
        return []
    return [Invocation(function="draw", input={"prompt": str(op.fields.get("description", op.asset)), "aspect_ratio": str(op.fields.get("aspect_ratio", "16:9"))})]
