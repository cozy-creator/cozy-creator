# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.38", "msgspec>=0.19"]
# ///
"""The project cell's picture package: a weightless still of one random colour, so each version differs. Its prompt says what it read."""

import os

from cozy_runtime.author import Adjustments, App, ImageFrame, ImageRequest, ImageResult, Invocation, Operation, Outputs, Sizes

app = App()
SIZES = Sizes((0.25, 4.0), (64, 8192), 16, "the still's 0.25-4 MP")


@app.entrypoint()
def draw(payload: ImageRequest, adj: Adjustments, out: Outputs) -> ImageResult:
    width, height = payload.size(adj, None, SIZES, megapixels=0.25)
    image = out.save_image(ImageFrame(width, height, os.urandom(3) * (width * height)), format="png")
    return ImageResult(image, width, height, payload.seed or 0, payload.prompt)


@app.project_adapter("draw")
def adapt(op: Operation) -> list[Invocation]:
    said = [f"[{read.name} " + " ".join(f"{role}={file.sha256}" for role, file in read.files.items()) + "]" for read in op.reads]
    return [Invocation(function="draw", input={"prompt": " ".join([str(op.inputs.get("instructions", op.asset)), *said])})]
