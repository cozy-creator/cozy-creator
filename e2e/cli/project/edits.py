# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime[media]>=0.45.0", "msgspec>=0.19"]
# ///
"""The rule-loras cell's tool (cl-1281): a still under one of two LoRAs, each a slot's default its rule over `edit` needs, as H3 Edit's five
are. They're the e2e Hub's (isolated.sh `loras`), whose cards hold their triggers: the page puts them in the prompt, the tool never does."""

import os
from typing import Annotated, Literal

import msgspec
from cozy_runtime.author import App, ImageFrame, Loader, Model, Seed
from cozy_runtime.fields import Image, Label, Prompt
from cozy_runtime.image import ImageResult
from cozy_runtime.media import ImageFile

app = App()
SIDE = 16


class Adapter(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Edit(msgspec.Struct, kw_only=True):
    edit: Annotated[
        Literal["vfx", "swap"],
        Label("Edit", options={"vfx": ("VFX edit", "Alissonerdx's VFX edit LoRA"), "swap": ("Body swap", "nrdx's body swap LoRA")}),
    ] = "vfx"
    summary: Prompt[Image] = Prompt()
    seed: Seed | None = None


@app.tool(label="Rule LoRAs", named="edited", accelerator=False,
          defaults={"vfx": "alissonerdx/minimax-h3-vfx-edit@0.1.0", "swap": "nrdx/bfs-body-swap-selective-lora@0.1.0"},
          needs={"vfx": 'request.edit == "vfx"', "swap": 'request.edit == "swap"'})  # fmt: skip
def edit(payload: Edit, vfx: Adapter, swap: Adapter) -> ImageResult:
    """A still of one colour, once its edit's LoRA is built."""
    _ = (vfx if payload.edit == "vfx" else swap).graph
    image = ImageFile.from_image(ImageFrame(SIDE, SIDE, os.urandom(3) * SIDE**2), format="png")
    return ImageResult(image, SIDE, SIDE, payload.seed or 0, payload.summary.text)
